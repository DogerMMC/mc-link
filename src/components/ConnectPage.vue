<script setup lang="ts">
import { ref, onMounted, onUnmounted } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

const props = defineProps<{
  showToast: (msg: string) => void;
}>();

const currentMode = ref<"none" | "running">("none");
const logs = ref<string[]>([]);
let unlistenLog: UnlistenFn | null = null;
let unlistenLatency: UnlistenFn | null = null;
const latencyMs = ref(0);
const roomName = ref("");
const roomPassword = ref("");
const isConnecting = ref(false);

onUnmounted(() => {
  if (unlistenLog) unlistenLog();
  if (unlistenLatency) unlistenLatency();
});

onMounted(async () => {
  unlistenLog = await listen<string>("app-log", (event) => {
    logs.value.push(event.payload);
    if (logs.value.length > 200) logs.value.shift();
  });

  unlistenLatency = await listen<number>("latency-update", (event) => {
    latencyMs.value = event.payload;
  });
});

async function startOnline() {
  if (!roomName.value || !roomPassword.value) {
    props.showToast("请填写房间名和密码");
    return;
  }

  if (currentMode.value === "running" || isConnecting.value) {
    props.showToast("联机功能已在运行中");
    return;
  }

  isConnecting.value = true;
  currentMode.value = "running";
  logs.value = [];
  latencyMs.value = 0;

  try {
    const selectedRelay = localStorage.getItem("relay_selected") || "__auto__";
    const result = await invoke("start_online", {
      roomName: roomName.value,
      password: roomPassword.value,
      selectedRelay,
    });
    props.showToast(result as string);
  } catch (e: any) {
    props.showToast(e.toString());
    currentMode.value = "none";
  } finally {
    isConnecting.value = false;
  }
}

async function stopOnline() {
  try {
    await invoke("stop_online");
    props.showToast("联机已停止");
    currentMode.value = "none";
    isConnecting.value = false;
    logs.value = [];
    latencyMs.value = 0;
  } catch (e) {
    props.showToast("停止失败: " + e);
  }
}

async function copyLogs() {
  if (logs.value.length === 0) {
    props.showToast("没有日志可复制");
    return;
  }
  try {
    await navigator.clipboard.writeText(logs.value.join("\n"));
    props.showToast("日志已复制到剪贴板");
  } catch (e) {
    props.showToast("复制失败: " + e);
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
  <div>
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
</template>

<style scoped>
.form, .connected {
  max-width: 400px;
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
</style>