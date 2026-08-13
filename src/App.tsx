import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";

type Device = { name: string; address: string; rssi: number };
type Progress = { sent: number; total: number };
type StorageInfo = { used: number; total: number };

type Screen = "connect" | "install" | "result";

function fmtMB(bytes: number): string {
  return (bytes / 1048576).toFixed(2) + " MB";
}

function App() {
  const [screen, setScreen] = useState<Screen>("connect");
  const [devices, setDevices] = useState<Device[]>([]);
  const [selected, setSelected] = useState<Device | null>(null);
  const [authkey, setAuthkey] = useState("");
  const [binPath, setBinPath] = useState("");
  const [storage, setStorage] = useState<StorageInfo | null>(null);
  const [progress, setProgress] = useState<Progress>({ sent: 0, total: 0 });
  const [logs, setLogs] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    const un = listen<Progress>("install:progress", (e) => {
      setProgress(e.payload);
      setLogs((prev) => [
        ...prev,
        `进度: ${e.payload.sent} / ${e.payload.total} 字节`,
      ]);
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  const doScan = async () => {
    setError(null);
    setBusy(true);
    try {
      const found = await invoke<Device[]>("scan_devices");
      setDevices(found);
      setLogs((prev) => [...prev, `扫描完成，发现 ${found.length} 个相关设备`]);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const doConnect = async () => {
    if (!selected) return;
    setError(null);
    setLogs([]);
    setBusy(true);
    try {
      await invoke("connect", { address: selected.address });
      setLogs((prev) => [...prev, `已连接 ${selected.address}（SPP RFCOMM）`]);
      await invoke("authenticate", { authkey });
      setLogs((prev) => [...prev, "认证成功"]);
      // 认证后查询手环存储
      try {
        const st = await invoke<StorageInfo>("get_storage_info");
        setStorage(st);
        setLogs((prev) => [
          ...prev,
          `存储: 已用 ${fmtMB(st.used)} / 共 ${fmtMB(st.total)}`,
        ]);
      } catch (e) {
        setLogs((prev) => [...prev, `存储查询失败: ${e}`]);
      }
      setScreen("install");
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const pickFile = async () => {
    try {
      const picked = await open({
        multiple: false,
        filters: [
          { name: "表盘文件", extensions: ["bin", "face"] },
          { name: "全部文件", extensions: ["*"] },
        ],
      });
      if (typeof picked === "string") {
        setBinPath(picked);
        setLogs((prev) => [...prev, `已选择文件: ${picked}`]);
      }
    } catch (e) {
      setError(String(e));
    }
  };

  const refreshStorage = async () => {
    try {
      const st = await invoke<StorageInfo>("get_storage_info");
      setStorage(st);
      setLogs((prev) => [
        ...prev,
        `存储: 已用 ${fmtMB(st.used)} / 共 ${fmtMB(st.total)}`,
      ]);
    } catch (e) {
      setLogs((prev) => [...prev, `存储查询失败: ${e}`]);
    }
  };

  const doInstall = async () => {
    setError(null);
    setLogs([]);
    setProgress({ sent: 0, total: 0 });
    setBusy(true);
    try {
      await invoke("install_watchface", { binPath });
      setLogs((prev) => [...prev, "安装完成"]);
      // 安装后刷新存储
      try {
        const st = await invoke<StorageInfo>("get_storage_info");
        setStorage(st);
        setLogs((prev) => [
          ...prev,
          `存储: 已用 ${fmtMB(st.used)} / 共 ${fmtMB(st.total)}`,
        ]);
      } catch {
        // 存储刷新失败不影响结果
      }
      setScreen("result");
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const doDisconnect = async () => {
    setError(null);
    try {
      await invoke("disconnect");
      setSelected(null);
      setScreen("connect");
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <main className="app">
      <h1>小米手环 10 Pro 表盘安装器</h1>

      {screen === "connect" && (
        <section>
          <div className="notice">
            ⚠️ 使用前请断开手机与手环的连接（蓝牙独占）；
            <br />
            若手环未绑定/恢复出厂，请先用官方 App 绑定并提取 authkey。
          </div>
          <div className="row">
            <button onClick={doScan} disabled={busy}>
              {busy ? "扫描中…" : "扫描设备"}
            </button>
          </div>
          <ul className="device-list">
            {devices.map((d) => (
              <li key={d.address}>
                <label>
                  <input
                    type="radio"
                    name="dev"
                    checked={selected?.address === d.address}
                    onChange={() => setSelected(d)}
                  />
                  {d.name} — {d.address} (rssi {d.rssi})
                </label>
              </li>
            ))}
          </ul>
          <input
            placeholder="authkey（hex，32 字符 = 16 字节）"
            value={authkey}
            onChange={(e) => setAuthkey(e.target.value)}
          />
          <div className="row">
            <button
              disabled={!selected || authkey.length === 0 || busy}
              onClick={doConnect}
            >
              连接并认证
            </button>
          </div>
        </section>
      )}

      {screen === "install" && (
        <section>
          {storage && (
            <div className="storage">
              手环存储：已用 {fmtMB(storage.used)} / 共 {fmtMB(storage.total)}
              （可用 {fmtMB(storage.total - storage.used)}）
              <button onClick={refreshStorage} disabled={busy}>
                刷新
              </button>
            </div>
          )}
          <div className="row">
            <input
              placeholder=".bin / .face 表盘文件路径"
              value={binPath}
              onChange={(e) => setBinPath(e.target.value)}
            />
            <button onClick={pickFile} disabled={busy}>
              选择文件…
            </button>
          </div>
          <div className="row">
            <button disabled={!binPath || busy} onClick={doInstall}>
              {busy ? "安装中…" : "安装"}
            </button>
            <button onClick={doDisconnect} disabled={busy}>
              断开
            </button>
          </div>
          <div className="progress">
            进度: {progress.sent} / {progress.total} 字节
            {progress.total > 0 && (
              <span>
                {" "}
                ({Math.round((progress.sent / progress.total) * 100)}%)
              </span>
            )}
          </div>
          <div className="logs">{logs.join("\n")}</div>
        </section>
      )}

      {screen === "result" && (
        <section>
          <h2>{error ? "安装失败" : "安装完成"}</h2>
          {error && <div className="error">{error}</div>}
          {!error && (
            <div className="success">表盘已推送，请在手环上查看表盘列表。</div>
          )}
          {storage && !error && (
            <div className="storage">
              手环存储：已用 {fmtMB(storage.used)} / 共 {fmtMB(storage.total)}
              （可用 {fmtMB(storage.total - storage.used)}）
            </div>
          )}
          <div className="row">
            <button onClick={() => setScreen("connect")}>返回</button>
          </div>
        </section>
      )}

      {error && screen !== "result" && <div className="error">{error}</div>}
    </main>
  );
}

export default App;
