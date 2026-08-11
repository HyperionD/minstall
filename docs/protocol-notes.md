# 手环 10 Pro BLE 协议笔记

> 本文件是 POC 阶段的核心产出。所有协议值必须来自真机验证或明确标注来源的参考实现，禁止臆造。

## 1. 参考实现来源
（Task 3 填充：仓库地址、获取日期、参考的设备型号）

## 2. 设备信息（真机）
- 固件版本：
- 表盘分辨率：

## 3. GATT 服务枚举结果（真机，scan.py 产出）
| Service UUID | Characteristic UUID | Properties | 疑似用途（对照参考实现） |
|---|---|---|---|

## 4. authkey 认证
- authkey 长度（hex 字符数）：
- 认证 service UUID：
- 认证 characteristic UUID（写/读/通知）：
- 握手流程（步骤序列 + 每步帧格式）：

## 5. 表盘推送
- 推送 service UUID：
- 推送 characteristic UUID：
- 分块大小：
- 帧格式（头部/序号/数据/校验）：
- 应答格式：
