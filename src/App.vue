<script setup lang="ts">
import { ref } from "vue";
import { getCurrentWindow } from '@tauri-apps/api/window';
import { invoke } from "@tauri-apps/api/core";
import IconSidebar from './components/IconSidebar.vue';
import ConnectPage from './components/ConnectPage.vue';
import RelayPage from './components/RelayPage.vue';

interface Toast {
  id: number;
  msg: string;
  isError: boolean;
}

const toasts = ref<Toast[]>([]);
let toastId = 0;

const activeIcon = ref("connect");

const iconItems = [
  { id: "connect", icon: "bi-wifi", title: "联机" },
  { id: "relay", icon: "bi-hdd-network", title: "中继" },
];

function handleIconChange(icon: string) {
  activeIcon.value = icon;
}

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
}
</script>

<template>
  <div class="window-wrapper">
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
        <button class="win-btn win-btn-close" @click="handleClose" title="关闭">
          <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
            <path d="M3 3L9 9M9 3L3 9" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>
          </svg>
        </button>
      </div>
    </div>
    <div class="window-container">
      <div class="content">
        <IconSidebar
          :active-icon="activeIcon"
          :icon-items="iconItems"
          @icon-change="handleIconChange"
        />
        <div class="page">
          <ConnectPage v-show="activeIcon === 'connect'" :show-toast="showToast" />
          <RelayPage v-show="activeIcon === 'relay'" :show-toast="showToast" />
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
@import url('https://fonts.googleapis.com/css2?family=Poppins:wght@300;400;500;600;700&display=swap');

:root {
  --bg-primary: #1e1e2e;
  --bg-secondary: #28283e;
  --bg-tertiary: #31314a;
  --bg-card: rgba(40, 40, 62);
  --bg-hover: rgba(255, 255, 255, 0.08);
  --bg-window: #222222;
  --text-primary: #ffffff;
  --text-secondary: #e0e0e0;
  --text-muted: #a0a0b0;
  --font-english: 'Poppins', sans-serif;
  --accent-primary: #0066cc;
  --accent-secondary: #0052a3;
  --accent-hover: #1a7ae6;
  --border-color: rgba(255, 255, 255, 0.08);
  --border-hover: rgba(255, 255, 255, 0.15);
  --shadow-accent: rgba(0, 102, 204, 0.25);
}

@media (prefers-color-scheme: light) {
  :root {
    --bg-primary: #F0F2F5;
    --bg-secondary: #FFFFFF;
    --bg-tertiary: #FFFFFF;
    --bg-card: #FFFFFF;
    --bg-hover: rgba(0, 0, 0, 0.05);
    --bg-window: #f5f5f5;
    --text-primary: #2c3e50;
    --text-secondary: #5a6c7d;
    --text-muted: #8b9bb0;
    --font-english: 'Poppins', sans-serif;
    --accent-primary: #0099ff;
    --accent-secondary: #0077cc;
    --accent-hover: #33adff;
    --border-color: #e8e8e8;
    --border-hover: #d0d0d0;
    --shadow-accent: rgba(0, 153, 255, 0.15);
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

.window-wrapper {
  width: 100vw;
  height: 100vh;
  background: transparent;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  position: relative;
}

.window-wrapper::before {
  content: '';
  position: absolute;
  inset: 0;
  background: rgba(0, 0, 0, 0.4);
  pointer-events: none;
  z-index: 0;
}

@media (prefers-color-scheme: light) {
  .window-wrapper::before {
    background: transparent;
  }
}

.window-wrapper > * {
  position: relative;
  z-index: 1;
}

.window-container {
  flex: 1;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  border-radius: 8px;
  margin: 0 5px 5px 5px;
  background: var(--bg-window);
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
  flex: 1;
  padding: 30px;
  overflow-y: auto;
  box-sizing: border-box;
}

.text-muted {
  color: var(--text-muted);
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
  box-shadow: 0 2px 8px rgba(0, 102, 204, 0.3);
}

.btn-primary:hover {
  background: var(--accent-hover);
}

.btn-danger {
  background: #ef4444;
  color: #ffffff;
  box-shadow: 0 2px 8px rgba(239, 68, 68, 0.3);
}

.btn-danger:hover {
  background: #f87171;
}

.toast-container {
  position: fixed;
  bottom: 15px;
  left: 15px;
  display: flex;
  flex-direction: column-reverse;
  gap: 6px;
  z-index: 1000;
}

.message {
  padding: 8px 16px;
  background: rgba(255, 255, 255, 0.1);
  color: var(--text-primary);
  border-radius: 6px;
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.2);
  max-width: 360px;
  backdrop-filter: blur(10px);
  font-size: 13px;
  line-height: 1.4;
}

.message.error {
  background: rgba(239, 68, 68, 0.2);
}

.toast-enter-active {
  transition: all 0.25s cubic-bezier(0.16, 1, 0.3, 1);
}

.toast-leave-active {
  transition: all 0.2s ease-out;
  position: absolute;
  right: auto;
  bottom: auto;
}

.toast-move {
  transition: transform 0.2s ease-out;
}

.toast-enter-from {
  opacity: 0;
  transform: translateX(-30px);
}

.toast-leave-to {
  opacity: 0;
  transform: translateX(-20px) scale(0.95);
}
</style>