<script setup lang="ts">
import { ref, onMounted, onUnmounted } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from '@tauri-apps/api/window';

const currentMode = ref<"none" | "running">("none");
const logs = ref<string[]>([]);
let unlistenLog: UnlistenFn | null = null;
let unlistenLatency: UnlistenFn | null = null;
const latencyMs = ref(0);
const roomName = ref("");
const roomPassword = ref("");
const isConnecting = ref(false);

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

onMounted(async () => {
  unlistenLog = await listen<string>("app-log", (event) => {
    logs.value.push(event.payload);
    if (logs.value.length > 200) logs.value.shift();
  });

  unlistenLatency = await listen<number>("latency-update", (event) => {
    latencyMs.value = event.payload;
  });

  const appWindow = getCurrentWindow();
  appWindow.onCloseRequested(async (event) => {
    event.preventDefault();
    await invoke("close_window");
    showToast("已最小化到系统托盘");
  });
});

onUnmounted(() => {
  if (unlistenLog) unlistenLog();
  if (unlistenLatency) unlistenLatency();
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
  await invoke("close_window");
  showToast("已最小化到系统托盘");
}

async function startOnline() {
  if (!roomName.value || !roomPassword.value) {
    showToast("请填写房间名和密码");
    return;
  }

  if (currentMode.value === "running" || isConnecting.value) {
    showToast("联机功能已在运行中");
    return;
  }

  isConnecting.value = true;
  currentMode.value = "running";
  toasts.value = [];
  logs.value = [];
  latencyMs.value = 0;

  try {
    const result = await invoke("start_online", {
      roomName: roomName.value,
      password: roomPassword.value
    });
    showToast(result as string);
  } catch (e: any) {
    showToast(e.toString());
    currentMode.value = "none";
  } finally {
    isConnecting.value = false;
  }
}

async function stopOnline() {
  try {
    await invoke("stop_online");
    showToast("联机已停止");
    currentMode.value = "none";
    isConnecting.value = false;
    logs.value = [];
    latencyMs.value = 0;
  } catch (e) {
    showToast("停止失败: " + e);
  }
}

async function copyLogs() {
  if (logs.value.length === 0) {
    showToast("没有日志可复制");
    return;
  }
  try {
    await navigator.clipboard.writeText(logs.value.join("\n"));
    showToast("日志已复制到剪贴板");
  } catch (e) {
    showToast("复制失败: " + e);
  }
}

function latencyColor(ms: number): string {
  if (ms === 0) return 'var(--text-muted)';
  if (ms <= 50) return '#22c55e';
  if (ms <= 100) return '#eab308';
  if (ms <= 200) return '#f97316';
  return '#ef4444';
}

function latencyLabel(ms: number): string {
  if (ms === 0) return '-- ms';
  if (ms >= 999) return '超时';
  return `${ms} ms`;
}
</script>

<template>
  <div class="main-container">
    <div class="titlebar" data-tauri-drag-region>
      <div class="titlebar-drag-region">
        MC Link
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
        <button class="win-btn win-btn-close" @click="handleClose" title="关闭到托盘">
          <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
            <path d="M3 3L9 9M9 3L3 9" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>
          </svg>
        </button>
      </div>
    </div>

    <div class="content">
      <div class="page">
        <h1 class="page-title">Minecraft 联机</h1>

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

        <div v-else class="connected">
          <div class="info-card">
            <p><strong>房间:</strong> {{ roomName }}</p>
            <p><strong>状态:</strong> 联机中</p>
            <p class="latency-row">
              <strong>延迟:</strong>
              <span class="latency-value" :style="{ color: latencyColor(latencyMs) }">
                {{ latencyLabel(latencyMs) }}
              </span>
            </p>
          </div>
          <button class="btn btn-danger" @click="stopOnline">停止联机</button>
        </div>

        <div v-show="logs.length > 0" class="console">
          <div class="console-header">
            <h3>运行日志</h3>
            <button class="copy-btn" @click="copyLogs">复制日志</button>
          </div>
          <div class="logs">
            <div v-for="(log, i) in logs" :key="i" class="log-line">{{ log }}</div>
          </div>
        </div>
      </div>
    </div>

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

.form, .connected {
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
  filter: brightness(1.1);
  box-shadow: 0 4px 12px rgba(129, 140, 248, 0.4);
}

.btn-danger {
  background: #ef4444;
  color: #ffffff;
  box-shadow: 0 2px 8px rgba(239, 68, 68, 0.3);
}

.btn-danger:hover {
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

.latency-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.latency-value {
  font-weight: 600;
  font-size: 14px;
  font-family: 'Consolas', monospace;
  transition: color 0.3s ease;
}

.console {
  margin-top: 30px;
  background: var(--bg-secondary);
  border-radius: 12px;
  overflow: hidden;
  border: 1px solid var(--border-color);
}

.console-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 12px 15px;
  background: var(--bg-tertiary);
}

.console-header h3 {
  margin: 0;
  color: var(--text-primary);
  font-size: 14px;
  font-weight: 600;
}

.copy-btn {
  background: var(--accent-primary);
  color: white;
  border: none;
  padding: 5px 12px;
  border-radius: 6px;
  font-size: 12px;
  cursor: pointer;
  transition: all 0.2s;
}

.copy-btn:hover {
  filter: brightness(1.1);
}

.log-line {
  color: var(--text-secondary);
  padding: 2px 0;
  border-bottom: 1px solid var(--border-color);
  user-select: text;
}

.logs {
  height: 250px;
  overflow-y: auto;
  padding: 10px;
  font-family: 'Consolas', monospace;
  font-size: 12px;
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