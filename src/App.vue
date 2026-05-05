<script setup lang="ts">
import { ref, onMounted, onUnmounted } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from '@tauri-apps/api/window';

const currentMode = ref<"none" | "running">("none");
const logs = ref<string[]>([]);
let unlistenLog: UnlistenFn | null = null;

interface Toast {
  id: number;
  msg: string;
  isError: boolean;
}

const toasts = ref<Toast[]>([]);
let toastId = 0;

function showToast(msg: string) {
  const isError = msg.includes('错误') || msg.includes('失败');

  if (toasts.value.length >= 3) {
    toasts.value.shift();
  }

  toasts.value.push({ id: ++toastId, msg, isError });

  setTimeout(() => {
    removeToast(toasts.value[0]?.id);
  }, 2000);
}

function removeToast(id: number) {
  const index = toasts.value.findIndex(t => t.id === id);
  if (index !== -1) {
    toasts.value.splice(index, 1);
  }
}

const roomName = ref("");
const roomPassword = ref("");
const isConnecting = ref(false);
const isRelayRunning = ref(false);
const isWindowFocused = ref(true);

onMounted(async () => {
  unlistenLog = await listen<string>("app-log", (event) => {
    logs.value.push(event.payload);
    if (logs.value.length > 200) logs.value.shift();
  });

  const appWindow = getCurrentWindow();
  appWindow.onFocusChanged(({ payload: focused }) => {
    isWindowFocused.value = focused;
  });
});

onUnmounted(() => {
  if (unlistenLog) unlistenLog();
});

async function handleMinimize() {
  try {
    const window = await getCurrentWindow();
    await window.minimize();
  } catch (error) {
    console.error('Minimize error:', error);
  }
}

async function handleMaximize() {
  try {
    const window = await getCurrentWindow();
    if (await window.isMaximized()) {
      await window.unmaximize();
    } else {
      await window.maximize();
    }
  } catch (error) {
    console.error('Maximize error:', error);
  }
}

async function handleClose() {
  try {
    const window = await getCurrentWindow();
    await window.close();
  } catch (error) {
    console.error('Close error:', error);
  }
}

async function startOnline() {
  if (!roomName.value || !roomPassword.value) {
    showToast("请填写房间名和密码");
    return;
  }

  isConnecting.value = true;
  toasts.value = [];
  logs.value = [];

  try {
    const result = await invoke("start_online", {
      roomName: roomName.value,
      password: roomPassword.value
    });
    showToast(result as string);
    currentMode.value = "running";
  } catch (e: any) {
    showToast(e.toString());
  } finally {
    isConnecting.value = false;
  }
}

async function stopOnline() {
  try {
    await invoke("stop_online");
    showToast("联机已停止");
    currentMode.value = "none";
    logs.value = [];
  } catch (e) {
    showToast("停止失败: " + e);
  }
}

async function toggleRelay() {
  try {
    if (!isRelayRunning.value) {
      const result = await invoke("start_relay_mode");
      showToast(result as string);
      isRelayRunning.value = true;
    } else {
      const result = await invoke("stop_relay_mode");
      showToast(result as string);
      isRelayRunning.value = false;
    }
  } catch (e) {
    showToast("中继服务器操作失败: " + e);
  }
}
</script>

<template>
  <div class="main-container" :class="{ unfocused: !isWindowFocused }">
    <!-- 自定义顶栏 -->
    <div class="titlebar">
      <div class="titlebar-drag-region" data-tauri-drag-region>
        MC-Link
      </div>
      <div class="window-controls">
        <button class="win-btn" @click="handleMinimize" title="最小化">
          <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
            <rect x="2" y="5.5" width="8" height="1" rx="0.5" fill="currentColor"/>
          </svg>
        </button>
        <button class="win-btn" @click="handleMaximize" title="最大化">
          <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
            <rect x="2" y="2" width="8" height="8" rx="1" stroke="currentColor" stroke-width="1" fill="none"/>
          </svg>
        </button>
        <button class="win-btn win-btn-close" @click="handleClose" title="关闭">
          <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
            <path d="M3 3L9 9M9 3L3 9" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>
          </svg>
        </button>
      </div>
    </div>

    <!-- 主内容区 -->
    <div class="content">
      <!-- 联机页面 -->
      <div class="page">
        <h1 class="page-title">Minecraft 联机</h1>

        <!-- 未连接状态 -->
        <div v-if="currentMode === 'none'" class="form">
          <div class="input-group">
            <label>房间名</label>
            <input type="text" v-model="roomName" placeholder="输入房间名" />
          </div>
          <div class="input-group">
            <label>密码</label>
            <input type="password" v-model="roomPassword" placeholder="输入密码" />
          </div>
          <div class="hint">
            房主：请先在Minecraft中开启局域网联机，然后创建房间<br>
            成员：输入房间名和密码加入即可加入
          </div>
          <button class="btn btn-primary" @click="startOnline" :disabled="isConnecting">
            {{ isConnecting ? '连接中...' : '开始联机' }}
          </button>
        </div>

        <!-- 已连接状态 -->
        <div v-else class="connected">
          <div class="info-card">
            <p><strong>房间:</strong> {{ roomName }}</p>
            <p><strong>状态:</strong> 联机中</p>
          </div>
          <button class="btn btn-danger" @click="stopOnline">停止联机</button>
        </div>

        <!-- 日志区域 -->
        <div v-if="logs.length > 0" class="console">
          <h3>运行日志</h3>
          <div class="logs">
            <div v-for="(log, i) in logs" :key="i" class="log-line">{{ log }}</div>
          </div>
        </div>
      </div>
    </div>

    <!-- 消息提示 -->
    <div class="toast-container">
      <TransitionGroup name="toast">
        <div
          v-for="toast in toasts"
          :key="toast.id"
          class="message"
          :class="{ error: toast.isError }"
        >
          {{ toast.msg }}
        </div>
      </TransitionGroup>
    </div>
  </div>
</template>

<style>
@import "tailwindcss";
@import "bootstrap-icons/font/bootstrap-icons.css";
@import url('https://fonts.googleapis.com/css2?family=Poppins:wght@300;400;500;600;700&display=swap');

:root {
  --bg-primary: #1e1e2e;
  --bg-primary-rgb: 30, 30, 46;
  --bg-secondary: #28283e;
  --bg-tertiary: #31314a;
  --bg-card: rgba(40, 40, 62);
  --bg-hover: rgba(255, 255, 255, 0.08);
  --text-primary: #ffffff;
  --text-secondary: #e0e0e0;
  --text-muted: #a0a0b0;
  --font-english: 'Poppins', sans-serif;
  --accent-primary: #818cf8;
  --accent-secondary: #6366f1;
  --accent-hover: #a5b4fc;
  --border-color: rgba(255, 255, 255, 0.08);
  --border-hover: rgba(255, 255, 255, 0.15);
  --shadow-accent: rgba(129, 140, 248, 0.2);
}

@media (prefers-color-scheme: light) {
  :root {
    --bg-primary: #F0F2F5;
    --bg-secondary: #FFFFFF;
    --bg-tertiary: #FFFFFF;
    --bg-card: #FFFFFF;
    --bg-hover: rgba(0, 0, 0, 0.05);
    --text-primary: #2c3e50;
    --text-secondary: #5a6c7d;
    --text-muted: #8b9bb0;
    --accent-primary: #1890FF;
    --accent-secondary: #096DD9;
    --accent-hover: #40A9FF;
    --border-color: #e8e8e8;
    --border-hover: #d0d0d0;
    --shadow-accent: rgba(24, 144, 255, 0.12);
  }
}

* {
  scrollbar-width: thin;
  scrollbar-color: var(--text-muted) transparent;
  font-family: var(--font-english), -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
}

body {
  background: transparent;
  margin: 0;
  padding: 0;
  overflow: hidden;
  user-select: none;
  -webkit-user-select: none;
}

.main-container {
  width: 100vw;
  height: 100vh;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  background: transparent;
}

.main-container.unfocused {
  background: #1e1e2e;
}

.main-container.unfocused .titlebar {
  background: #1e1e2e;
}

@media (prefers-color-scheme: light) {
  .main-container.unfocused {
    background: #f0f2f5;
  }
  .main-container.unfocused .titlebar {
    background: #f0f2f5;
  }
}

.titlebar {
  height: 36px;
  background: transparent;
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 0 16px;
  user-select: none;
  box-sizing: border-box;
}

.titlebar-drag-region {
  flex: 1;
  height: 100%;
  display: flex;
  align-items: center;
  -webkit-app-region: drag;
  cursor: default;
  color: var(--text-secondary);
  font-size: 13px;
  font-weight: 500;
}

.window-controls {
  display: flex;
  align-items: center;
  -webkit-app-region: no-drag;
  z-index: 100;
}

.win-btn {
  width: 36px;
  height: 28px;
  border: none;
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  transition: all 0.15s ease;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: 4px;
  margin-left: 4px;
}

.win-btn:hover {
  background: rgba(255, 255, 255, 0.1);
  color: var(--text-primary);
}

.win-btn:active {
  transform: scale(0.95);
  background: rgba(255, 255, 255, 0.05);
}

.win-btn-close:hover {
  background: rgba(239, 68, 68, 0.8);
  color: white;
}

.win-btn:hover {
  background: var(--bg-hover);
}

.content {
  flex: 1;
  display: flex;
  overflow: hidden;
}

.page {
  padding: 30px;
  overflow-y: auto;
  height: 100%;
  box-sizing: border-box;
}

.page-title {
  margin: 0 0 30px;
  color: var(--text-primary);
  font-size: 24px;
  font-weight: 600;
}

.form {
  max-width: 400px;
}

.connected {
  max-width: 400px;
}

.input-group {
  margin-bottom: 20px;
}

.input-group label {
  display: block;
  margin-bottom: 8px;
  color: var(--text-secondary);
  font-weight: 500;
  font-size: 14px;
}

.input-group input {
  width: 100%;
  padding: 12px;
  border: none;
  border-radius: 8px;
  font-size: 14px;
  box-sizing: border-box;
  background: rgba(255, 255, 255, 0.1);
  color: var(--text-primary);
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.15);
  transition: all 0.2s ease;
}

.main-container.unfocused .input-group input {
  background: var(--bg-secondary);
  box-shadow: none;
}

.input-group input:focus {
  outline: none;
  background: rgba(255, 255, 255, 0.15);
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.2);
}

.hint {
  margin: 15px 0;
  color: var(--text-primary);
  font-size: 13px;
  line-height: 1.6;
}

.btn {
  transition: all 0.2s ease;
  border-radius: 8px;
  cursor: pointer;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  font-size: 14px;
  font-weight: 500;
  padding: 12px 24px;
  border: none;
  background: rgba(255, 255, 255, 0.1);
  color: var(--text-primary);
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.15);
}

.btn:hover {
  background: rgba(255, 255, 255, 0.15);
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.2);
}

.main-container.unfocused .btn {
  background: var(--bg-tertiary);
  box-shadow: none;
}

.main-container.unfocused .btn:hover {
  background: var(--bg-hover);
  box-shadow: none;
}

.main-container.unfocused .btn-primary {
  background: var(--accent-primary);
}

.main-container.unfocused .btn-primary:hover {
  filter: brightness(1.1);
}

.main-container.unfocused .btn-danger {
  background: #ef4444;
}

.main-container.unfocused .btn-danger:hover {
  filter: brightness(1.1);
}

.btn:active {
  transform: scale(0.98);
  transition: transform 0.1s ease;
}

.btn:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}

.btn-primary {
  background: var(--accent-primary);
  color: #ffffff;
  box-shadow: 0 2px 8px rgba(129, 140, 248, 0.3);
}

.btn-primary:hover {
  background: var(--accent-primary);
  color: #ffffff;
  filter: brightness(1.1);
  box-shadow: 0 4px 12px rgba(129, 140, 248, 0.4);
}

.btn-danger {
  background: #ef4444;
  color: #ffffff;
  box-shadow: 0 2px 8px rgba(239, 68, 68, 0.3);
}

.btn-danger:hover {
  background: #ef4444;
  color: #ffffff;
  filter: brightness(1.1);
  box-shadow: 0 4px 12px rgba(239, 68, 68, 0.4);
}

.info-card {
  background: var(--bg-card);
  padding: 20px;
  border-radius: 12px;
  margin-bottom: 20px;
  border: 1px solid var(--border-color);
}

.info-card p {
  margin: 10px 0;
  color: var(--text-secondary);
}

.info-card strong {
  color: var(--text-primary);
}

.console {
  margin-top: 30px;
  background: var(--bg-secondary);
  border-radius: 12px;
  overflow: hidden;
  border: 1px solid var(--border-color);
}

.console h3 {
  margin: 0;
  padding: 12px 15px;
  background: var(--bg-tertiary);
  color: var(--text-primary);
  font-size: 14px;
  font-weight: 600;
}

.logs {
  height: 250px;
  overflow-y: auto;
  padding: 10px;
  font-family: 'Consolas', monospace;
  font-size: 12px;
}

.log-line {
  color: var(--text-secondary);
  padding: 2px 0;
  border-bottom: 1px solid var(--border-color);
}

.toast-container {
  position: fixed;
  bottom: 30px;
  right: 30px;
  display: flex;
  flex-direction: column-reverse;
  gap: 10px;
  z-index: 1000;
}

.message {
  padding: 15px 25px;
  background: rgba(255, 255, 255, 0.1);
  color: var(--text-primary);
  border-radius: 8px;
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.2);
  max-width: 400px;
  backdrop-filter: blur(10px);
}

.message.error {
  background: rgba(239, 68, 68, 0.2);
}

.main-container.unfocused .message {
  background: var(--bg-card);
  backdrop-filter: none;
  box-shadow: none;
}

.toast-enter-active {
  animation: slideIn 0.3s ease-out;
}

.toast-leave-active {
  animation: fadeOut 0.3s ease-out forwards;
  z-index: 0;
}

.toast-move {
  transition: transform 0.5s ease-in;
}

@keyframes slideIn {
  from {
    opacity: 0;
    transform: translateX(50px);
  }
  to {
    opacity: 1;
    transform: translateX(0);
  }
}

@keyframes fadeOut {
  from {
    opacity: 1;
  }
  to {
    opacity: 0;
  }
}
</style>
