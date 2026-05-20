# MC Link

我的世界联机工具 — 通过中继服务器实现远程联机，无需公网 IP，无需端口映射，无需内网穿透。

## 架构

```
房主 Minecraft ──→ MC Link 房主 ──→ 中继服务器 ←── MC Link 成员 ←── 成员 Minecraft
                                        ↕
                                 中央服务器 (房间协调/中继列表)
```

- **中央服务器** (central-server)：管理房间和中继服务器列表
- **中继服务器** (mc-link-relay)：转发游戏数据，负责连接双方
- **客户端** (src-tauri)：Tauri 桌面应用，包含房主模式和成员模式

## 快速开始

### 下载客户端

从 [Releases](https://github.com/DogerMMC/mc-link/releases) 下载最新版本 `mc-link-v0.x.x.exe`，直接运行即可。

### 启动服务端（自建中继）

```bash
# 中央服务器
cd central-server
cargo build --release
./target/release/mc-link-central.exe

# 中继服务器（需先修改 config.yml 中的 central_server 地址）
cd mc-link-relay
cargo build --release
./target/release/mc-link-relay.exe
```

### 使用

1. **房主**：启动 MC Link → 点击"创建房间"→ 选房间 → 点"开始联机"
2. **成员**：启动 MC Link → 输入房间名和密码 → 点"开始联机"→ 打开 Minecraft 连接显示的地址

## 技术栈

- **客户端**: Tauri 2 + Vue 3 + TypeScript + Rust
- **中央服务器**: Rust (TCP)
- **中继服务器**: Rust (TCP + AES-256-ECB 加密)

## 协议

中继协议使用自定义加密 TCP 协议：
- 包头：`[room_len:1byte][room_name][pass_len:1byte][password][AES加密数据]`
- AES-256-ECB 加密，密钥由密码的 SHA256 派生
- TCP 帧：4 字节大端长度前缀 + 数据

## 版本历史

| 版本 | 说明 |
|------|------|
| 0.2.8 | 自动寻找可用本地端口 |
| 0.2.7 | MC_READY 通知机制，等成员MC连上后再连房主 |
| 0.2.6 | 移除 WebRTC，托盘菜单适配深浅色模式 |
| 0.2.5 | 修复中继连接非阻塞模式导致断开 |
| 0.2.4 | 修复热键冲突导致崩溃 |
| 0.2.3 | 初始中继架构 |