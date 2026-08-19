import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";

type Device = { name: string; address: string; rssi: number };
type Progress = { sent: number; total: number };
type StorageInfo = { used: number; total: number };
type PickerResult =
  | { status: "pending" | "missing" }
  | { status: "selected"; path: string }
  | { status: "cancelled" }
  | { status: "error"; message: string };

type Screen = "connect" | "install";
type InstallKind = "watchface" | "quickapp";
type WatchState = "idle" | "busy" | "progress" | "ok" | "error";

function isQuickAppPath(path: string): boolean {
  return /\.rpk$/i.test(path);
}

function fmtMB(bytes: number): string {
  return (bytes / 1048576).toFixed(2) + " MB";
}

function fmtBytes(bytes: number): string {
  if (bytes >= 1048576) return (bytes / 1048576).toFixed(1) + " MB";
  if (bytes >= 1024) return (bytes / 1024).toFixed(0) + " KB";
  return bytes + " B";
}

/** 圆形表盘半径 / 周长 */
const FACE_R = 84;
const FACE_C = 2 * Math.PI * FACE_R;

/**
 * 签名元素：仿手环 OLED 屏的圆形表盘。
 * 状态、进度都通过它表达：环形进度 + 刻度 + 中心状态。
 */
function WatchFace({
  state,
  status,
  pct,
}: {
  state: WatchState;
  status: string;
  pct: number;
}) {
  const ticks = [];
  for (let i = 0; i < 24; i++) {
    const major = i % 6 === 0;
    const angle = (i / 24) * Math.PI * 2 - Math.PI / 2;
    const r1 = major ? 92 : 89;
    const r2 = 94;
    ticks.push(
      <line
        key={i}
        x1={100 + r1 * Math.cos(angle)}
        y1={100 + r1 * Math.sin(angle)}
        x2={100 + r2 * Math.cos(angle)}
        y2={100 + r2 * Math.sin(angle)}
        className={major ? "face-tick face-tick--major" : "face-tick"}
      />
    );
  }
  const clamped = Math.max(0, Math.min(100, pct));
  const ringOffset = FACE_C * (1 - clamped / 100);

  return (
    <div className={`watchface watchface--${state}`} role="status" aria-label={status}>
      <svg viewBox="0 0 200 200" className="watchface__svg">
        <circle className="face-ring face-ring--track" cx="100" cy="100" r={FACE_R} />
        <circle
          className="face-ring face-ring--value"
          cx="100"
          cy="100"
          r={FACE_R}
          strokeDasharray={FACE_C}
          strokeDashoffset={ringOffset}
        />
        {ticks}
      </svg>
      <div className="watchface__content">
        <div className="watchface__pct">
          {state === "progress" ? `${Math.round(clamped)}%` : "—"}
        </div>
        <div className="watchface__status">{status}</div>
      </div>
    </div>
  );
}

function App() {
  const [screen, setScreen] = useState<Screen>("connect");
  const [devices, setDevices] = useState<Device[]>([]);
  const [selected, setSelected] = useState<Device | null>(null);
  const [manualMac, setManualMac] = useState("");
  const [authkey, setAuthkey] = useState("");
  const [rememberAuthkey, setRememberAuthkey] = useState(false);
  const [binPath, setBinPath] = useState("");
  const [installKind, setInstallKind] = useState<InstallKind>("watchface");
  const [storage, setStorage] = useState<StorageInfo | null>(null);
  const [progress, setProgress] = useState<Progress>({ sent: 0, total: 0 });
  const [logs, setLogs] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [picking, setPicking] = useState(false);
  const pickInFlight = useRef(false);
  const pickerRequestId = useRef(0);
  const activePickerRequest = useRef<number | null>(null);
  const [watchState, setWatchState] = useState<WatchState>("idle");
  const [watchStatus, setWatchStatus] = useState("等待连接");

  const logRef = useRef<HTMLDivElement>(null);
  const isAndroid = /Android/i.test(navigator.userAgent);

  const setLogsAuto = (fn: (prev: string[]) => string[]) => {
    setLogs(fn);
  };

  const applyPickedFile = (picked: string) => {
    setBinPath(picked);
    setInstallKind(isQuickAppPath(picked) ? "quickapp" : "watchface");
    setLogsAuto((prev) => [...prev, `已选择文件: ${picked}`]);
  };

  // 日志自动滚到底
  useEffect(() => {
    if (logRef.current) {
      logRef.current.scrollTop = logRef.current.scrollHeight;
    }
  }, [logs]);

  // 优先从系统安全存储加载 authkey；Android 无保存值时再尝试自动检测。
  useEffect(() => {
    let cancelled = false;
    const loadAuthkey = async () => {
      let loaded = false;
      try {
        const saved = await invoke<string | null>("get_saved_authkey");
        if (!cancelled && saved) {
          setAuthkey(saved);
          setRememberAuthkey(true);
          setSuccess("已从系统安全存储加载 authkey");
          loaded = true;
        }
      } catch {
        /* 当前平台没有可用的已保存 authkey 时静默继续 */
      }
      if (cancelled || loaded || !isAndroid) return;
      try {
        const val = await invoke<string>("read_authkey");
        if (!cancelled && val.startsWith("FOUND|")) {
          setAuthkey(val.slice(6));
          setSuccess("已自动读取 authkey");
        }
      } catch {
        /* 自动读取失败不打扰用户，可手动输入 */
      }
    };
    void loadAuthkey();
    return () => {
      cancelled = true;
    };
  }, [isAndroid]);

  // 记住上次连接：MAC 地址可普通持久化，authkey 只经过系统安全存储。
  useEffect(() => {
    try {
      const mac = localStorage.getItem("minstall.lastMac");
      localStorage.removeItem("minstall.lastAuthkey");
      if (mac) setManualMac(mac);
    } catch {
      /* 读取失败忽略 */
    }
  }, []);

  useEffect(() => {
    const un = listen<Progress>("install:progress", (e) => {
      setProgress(e.payload);
      setWatchState("progress");
      setWatchStatus(
        `传输中 ${fmtBytes(e.payload.sent)} / ${fmtBytes(e.payload.total)}`
      );
      setLogsAuto((prev) => [
        ...prev,
        `进度: ${e.payload.sent} / ${e.payload.total} 字节`,
      ]);
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  useEffect(() => {
    if (!isAndroid || !picking) return;
    const requestId = activePickerRequest.current;
    if (requestId === null) return;
    let stopped = false;
    let polling = false;
    let missingCount = 0;
    const startedAt = Date.now();

    const finish = (result: PickerResult) => {
      if (stopped || activePickerRequest.current !== requestId) return;
      if (result.status === "selected") applyPickedFile(result.path);
      if (result.status === "error") setError(result.message);
      activePickerRequest.current = null;
      pickInFlight.current = false;
      setPicking(false);
      void invoke("ack_file_picker_result", { requestId });
    };

    const poll = async () => {
      if (stopped || polling || document.hidden) return;
      if (Date.now() - startedAt > 120_000) {
        finish({ status: "error", message: "文件选择超时，请重试" });
        return;
      }
      polling = true;
      try {
        const result = await Promise.race([
          invoke<PickerResult>("get_file_picker_result", { requestId }),
          new Promise<never>((_, reject) =>
            window.setTimeout(() => reject(new Error("查询文件选择结果超时")), 2_000)
          ),
        ]);
        if (result.status === "pending") missingCount = 0;
        if (result.status === "missing") {
          missingCount += 1;
          if (missingCount < 40) return;
          finish({ status: "error", message: "未找到文件选择请求，请重试" });
          return;
        }
        if (result.status !== "pending") finish(result);
      } catch {
        // IPC 可能在系统选择器切换 Activity 时丢失，下一轮继续查询。
      } finally {
        polling = false;
      }
    };

    const timer = window.setInterval(() => void poll(), 250);
    const onVisibilityChange = () => {
      if (!document.hidden) void poll();
    };
    document.addEventListener("visibilitychange", onVisibilityChange);
    void poll();
    return () => {
      stopped = true;
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisibilityChange);
    };
  }, [isAndroid, picking]);

  const doScan = async () => {
    setError(null);
    setWatchState("busy");
    setWatchStatus("扫描中…");
    setBusy(true);
    try {
      const found = await invoke<Device[]>("scan_devices");
      setDevices(found);
      setWatchState("idle");
      setWatchStatus("等待连接");
      setLogsAuto((prev) => [...prev, `扫描完成，发现 ${found.length} 个相关设备`]);
    } catch (e) {
      setError(String(e));
      setWatchState("error");
      setWatchStatus("扫描失败");
    } finally {
      setBusy(false);
    }
  };

  const doConnect = async () => {
    const address = selected?.address ?? manualMac.trim();
    if (!address) return;
    setError(null);
    setSuccess(null);
    setLogs([]);
    setBusy(true);
    setWatchState("busy");
    setWatchStatus("连接中…");
    try {
      await invoke("connect", { address });
      setLogsAuto((prev) => [...prev, `已连接 ${address}（SPP RFCOMM）`]);
      setWatchStatus("认证中…");
      await invoke("authenticate", { authkey });
      setLogsAuto((prev) => [...prev, "认证成功"]);
      // MAC 可普通持久化；authkey 仅在用户同意时写入系统安全存储。
      try {
        localStorage.setItem("minstall.lastMac", address);
      } catch {
        /* MAC 保存失败不影响连接 */
      }
      try {
        if (rememberAuthkey) {
          await invoke("save_authkey", { authkey });
          setLogsAuto((prev) => [...prev, "authkey 已保存到系统安全存储"]);
        } else {
          await invoke("clear_saved_authkey");
        }
      } catch (e) {
        setError(`认证成功，但保存 authkey 失败：${e}`);
      }
      setWatchState("ok");
      setWatchStatus("已认证 · 就绪");
      // 认证后查询手环存储
      try {
        const st = await invoke<StorageInfo>("get_storage_info");
        setStorage(st);
        setLogsAuto((prev) => [
          ...prev,
          `存储: 已用 ${fmtMB(st.used)} / 共 ${fmtMB(st.total)}`,
        ]);
      } catch (e) {
        setLogsAuto((prev) => [...prev, `存储查询失败: ${e}`]);
      }
      setScreen("install");
    } catch (e) {
      setError(String(e));
      setWatchState("error");
      setWatchStatus("连接失败");
    } finally {
      setBusy(false);
    }
  };

  const pickFile = async () => {
    if (pickInFlight.current) return;
    pickInFlight.current = true;
    setPicking(true);

    if (isAndroid) {
      const requestId = ++pickerRequestId.current;
      activePickerRequest.current = requestId;
      void invoke("start_file_picker", { requestId }).catch((reason) => {
        if (activePickerRequest.current !== requestId) return;
        setError(String(reason));
        activePickerRequest.current = null;
        pickInFlight.current = false;
        setPicking(false);
      });
      return;
    }

    try {
      const picked = await open({
        multiple: false,
        filters: [
          { name: "表盘文件", extensions: ["bin", "face"] },
          { name: "快应用包", extensions: ["rpk"] },
          { name: "全部文件", extensions: ["*"] },
        ],
      });
      if (typeof picked === "string") applyPickedFile(picked);
    } catch (reason) {
      setError(String(reason));
    } finally {
      pickInFlight.current = false;
      setPicking(false);
    }
  };

  const refreshStorage = async () => {
    try {
      const st = await invoke<StorageInfo>("get_storage_info");
      setStorage(st);
      setLogsAuto((prev) => [
        ...prev,
        `存储: 已用 ${fmtMB(st.used)} / 共 ${fmtMB(st.total)}`,
      ]);
    } catch (e) {
      setLogsAuto((prev) => [...prev, `存储查询失败: ${e}`]);
    }
  };

  const doInstall = async () => {
    setError(null);
    setSuccess(null);
    setLogs([]);
    setProgress({ sent: 0, total: 0 });
    setBusy(true);
    setWatchState("busy");
    setWatchStatus("准备安装…");
    try {
      const outcome = installKind === "quickapp"
        ? await invoke<"confirmed" | "transferred">("install_quick_app", {
            rpkPath: binPath,
          })
        : await invoke<"confirmed" | "transferred">("install_watchface", {
            binPath,
          });
      const name = binPath.split(/[\\/]/).pop();
      const label = installKind === "quickapp" ? "快应用" : "表盘";
      if (outcome === "confirmed") {
        setLogsAuto((prev) => [...prev, `${label}安装完成（手环已确认）`]);
        setSuccess(`${label}安装成功：${name}`);
        setWatchState("ok");
        setWatchStatus("安装成功");
      } else {
        setLogsAuto((prev) => [
          ...prev,
          `${label}传输完成，请在手环上确认是否安装成功`,
        ]);
        setSuccess(`${label}传输成功：${name}，请在手环上确认`);
        setWatchState("ok");
        setWatchStatus("传输完成");
      }
      // 安装后刷新存储
      try {
        const st = await invoke<StorageInfo>("get_storage_info");
        setStorage(st);
        setLogsAuto((prev) => [
          ...prev,
          `存储: 已用 ${fmtMB(st.used)} / 共 ${fmtMB(st.total)}`,
        ]);
      } catch {
        // 存储刷新失败不影响结果
      }
    } catch (e) {
      setError(String(e));
      setWatchState("error");
      setWatchStatus("安装失败");
    } finally {
      setBusy(false);
    }
  };

  const doDisconnect = async () => {
    setError(null);
    setSuccess(null);
    setWatchState("busy");
    setWatchStatus("断开中…");
    try {
      await invoke("disconnect");
      setSelected(null);
      setBinPath("");
      setInstallKind("watchface");
      setStorage(null);
      setProgress({ sent: 0, total: 0 });
      setScreen("connect");
      setWatchState("idle");
      setWatchStatus("等待连接");
    } catch (e) {
      setError(String(e));
    }
  };

  const pct =
    progress.total > 0 ? Math.round((progress.sent / progress.total) * 100) : 0;

  return (
    <main className="app">
      <header className="masthead">
        <div className="masthead__brand">
          <span className="masthead__logo">◉</span>
          <div>
            <h1>minstall</h1>
            <p className="masthead__sub">小米手环 10 Pro · 表盘 / 快应用直装</p>
          </div>
        </div>
        <div className={`conn-badge conn-badge--${screen === "install" ? "on" : "off"}`}>
          <span className="conn-badge__dot" />
          {screen === "install" ? "已连接" : "未连接"}
        </div>
      </header>

      <WatchFace state={watchState} status={watchStatus} pct={pct} />

      {error && <div className="alert alert--error">{error}</div>}
      {success && <div className="alert alert--success">{success}</div>}

      {screen === "connect" && (
        <section className="card">
          <div className="step-head">
            <span className="step-head__no">1</span>
            <div>
              <h2>连接手环</h2>
              <p className="step-head__hint">
                支持安装 .bin / .face 表盘和 .rpk Vela 快应用。
                <br />
                若手环未绑定，请先用官方 App 绑定并提取 authkey；可点「自动检测」从导出日志读取 authkey，或手动输入 32 位 hex。
              </p>
            </div>
          </div>

          <div className="field">
            <label className="field__label">选择设备</label>
            <button
              className="btn btn--ghost btn--block"
              onClick={doScan}
              disabled={busy}
            >
              {busy ? "扫描中…" : "扫描手环"}
            </button>
            {devices.length === 0 && !busy ? (
              <p className="empty-hint">
                未发现手环。点击「扫描手环」，或在下方向手动输入 MAC 地址。
              </p>
            ) : (
              <ul className="device-list">
                {devices.map((d) => (
                  <li
                    key={d.address}
                    className={
                      selected?.address === d.address ? "device-list__item is-selected" : "device-list__item"
                    }
                  >
                    <label>
                      <input
                        type="radio"
                        name="dev"
                        checked={selected?.address === d.address}
                        onChange={() => setSelected(d)}
                      />
                      <span className="device-list__name">{d.name}</span>
                      <span className="device-list__addr">{d.address}</span>
                      <span className="device-list__rssi">{d.rssi} dBm</span>
                    </label>
                  </li>
                ))}
              </ul>
            )}
          </div>

          <div className="field">
            <label className="field__label" htmlFor="manual-mac">
              或手动输入 MAC
            </label>
            <input
              id="manual-mac"
              className="input input--mono"
              placeholder="2C:0D:CF:73:D9:95"
              value={manualMac}
              onChange={(e) => setManualMac(e.target.value)}
              spellCheck={false}
            />
          </div>

          <div className="field">
            <label className="field__label" htmlFor="authkey">
              authkey（32 位 hex）
            </label>
            <div className="input-wrap">
              <input
                id="authkey"
                className="input input--mono"
                type="text"
                placeholder="xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
                value={authkey}
                onChange={(e) => setAuthkey(e.target.value)}
                spellCheck={false}
              />
              {isAndroid && (
                <button
                  className="btn btn--ghost"
                  onClick={async () => {
                    setError(null);
                    try {
                      const val = await invoke<string>("read_authkey");
                      if (val.startsWith("FOUND|")) {
                        setAuthkey(val.slice(6));
                        setSuccess("已自动读取 authkey");
                        setError(null);
                      } else if (val === "DIR_MISSING") {
                        setError(
                          "未找到日志目录。请在小米运动健康 App：我的 → 关于 → 连续点击界面最上方的 App 图标 → 弹出对话框点「确定」导出日志，完成后再点「自动检测」。"
                        );
                      } else if (val === "NEED_PERMISSION") {
                        setError("需要「所有文件访问」权限才能读取日志，正在打开设置…");
                        await invoke("open_storage_permission_settings");
                      } else {
                        setError(
                          "未检测到 authkey：请先在小米运动健康中导出日志，或手动输入"
                        );
                      }
                    } catch (e) {
                      setError(String(e));
                    }
                  }}
                  type="button"
                >
                  自动检测
                </button>
              )}
            </div>
            <p className="field__note">
              {isAndroid
                ? "自动检测：从剪贴板读取，或扫描小米运动健康导出的日志 zip；也可手动输入"
                : "Linux 桌面端请手动输入从官方 App 导出的 authkey"}
            </p>
            <label className="secure-option">
              <input
                type="checkbox"
                checked={rememberAuthkey}
                onChange={async (event) => {
                  const checked = event.target.checked;
                  setRememberAuthkey(checked);
                  if (!checked) {
                    try {
                      await invoke("clear_saved_authkey");
                      setSuccess("已清除系统中保存的 authkey");
                    } catch (e) {
                      setError(`清除已保存 authkey 失败：${e}`);
                    }
                  }
                }}
              />
              <span>
                记住 authkey
                <small>仅保存在系统安全存储中，不写入浏览器本地存储</small>
              </span>
            </label>
          </div>

          <button
            className="btn btn--primary btn--block"
            disabled={(!selected && !manualMac.trim()) || authkey.length === 0 || busy}
            onClick={doConnect}
          >
            {busy ? "连接中…" : "连接并认证"}
          </button>
        </section>
      )}

      {screen === "install" && (
        <section className="card">
          <div className="step-head">
            <span className="step-head__no">2</span>
            <div>
              <h2>{installKind === "quickapp" ? "安装快应用" : "安装表盘"}</h2>
              <p className="step-head__hint">
                {installKind === "quickapp"
                  ? "选择 .rpk Vela 快应用包并安装到手环"
                  : "支持 .bin / .face 表盘和 .rpk Vela 快应用，选择文件后自动识别安装类型"}
              </p>
            </div>
          </div>

          {storage && (
            <div className="storage">
              <div className="storage__top">
                <span>手环存储</span>
                <button className="btn btn--ghost btn--sm" onClick={refreshStorage} disabled={busy}>
                  刷新
                </button>
              </div>
              <div className="storage__bar">
                <div
                  className="storage__fill"
                  style={{
                    width: `${Math.min(100, (storage.used / storage.total) * 100)}%`,
                  }}
                />
              </div>
              <div className="storage__meta">
                <span>
                  已用 {fmtMB(storage.used)} / 共 {fmtMB(storage.total)}
                </span>
                <span>可用 {fmtMB(storage.total - storage.used)}</span>
              </div>
            </div>
          )}

          <div className="field">
            <label className="field__label" htmlFor="bin-path">
              {installKind === "quickapp" ? "快应用包" : "表盘文件"}
            </label>
            <div className="input-wrap">
              <input
                id="bin-path"
                className="input input--mono"
                placeholder={
                  installKind === "quickapp"
                    ? "选择或输入 .rpk 文件路径"
                    : "选择或输入 .bin / .face / .rpk 文件路径"
                }
                value={binPath}
                onChange={(e) => {
                  setBinPath(e.target.value);
                  setInstallKind(isQuickAppPath(e.target.value) ? "quickapp" : "watchface");
                }}
                spellCheck={false}
              />
              <button className="btn btn--ghost" onClick={pickFile} disabled={busy || picking}>
                {picking ? "选择器打开中…" : "选择文件…"}
              </button>
            </div>
          </div>

          <div className="row">
            <button
              className="btn btn--primary btn--grow"
              disabled={!binPath || busy}
              onClick={doInstall}
            >
              {busy ? "安装中…" : installKind === "quickapp" ? "安装快应用" : "安装表盘"}
            </button>
            <button className="btn btn--ghost" onClick={doDisconnect} disabled={busy}>
              断开连接
            </button>
          </div>

          <div className="log-term" ref={logRef}>
            {logs.length === 0 ? (
              <span className="log-term__placeholder">
                — 操作日志将显示在这里 —
              </span>
            ) : (
              logs.map((l, i) => (
                <div key={i} className="log-term__line">
                  {l}
                </div>
              ))
            )}
          </div>
        </section>
      )}

      <footer className="foot">
        minstall · 蓝牙直装，不经过官方 App · 开源工具，请谨慎操作
      </footer>
    </main>
  );
}

export default App;
