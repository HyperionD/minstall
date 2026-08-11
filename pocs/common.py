"""POC 脚本公共工具：日志走 stderr，结构化结果走 stdout。"""
import json
import sys
import time


def log(msg: str) -> None:
    print(f"[{time.strftime('%H:%M:%S')}] {msg}", file=sys.stderr, flush=True)


def emit_json(obj) -> None:
    """向 stdout 输出一行 JSON，供脚本链式解析。"""
    print(json.dumps(obj, ensure_ascii=False), flush=True)
