<script setup lang="ts">
import { ref, computed, watch, onMounted } from "vue";
import { invoke } from "@tauri-apps/api/core";

const props = defineProps<{
  showToast: (msg: string) => void;
}>();

interface RelayInfo {
  id: string;
  name: string;
  address: string;
}

interface IpInfo {
  region: string;
  isp: string;
}

const AUTO_ID = "__auto__";

const relayList = ref<RelayInfo[]>([]);
const searchQuery = ref("");
const selectedRelay = ref<string | null>(localStorage.getItem("relay_selected") || AUTO_ID);
watch(selectedRelay, (val) => {
  if (val) localStorage.setItem("relay_selected", val);
});
const customRelay = ref<{ address: string; latency: number; pinging: boolean } | null>(null);
const ipInfoMap = ref<Record<string, IpInfo>>({});
let pingTimer: ReturnType<typeof setTimeout> | null = null;

interface RelayEntry {
  id: string;
  name: string;
  address: string;
  isAuto: boolean;
}

const autoEntry: RelayEntry = { id: AUTO_ID, name: "自动选择", address: "自动选择最优中继节点", isAuto: true };

const filteredEntries = computed(() => {
  const q = searchQuery.value.toLowerCase();
  const matches = (entry: RelayEntry) => {
    if (entry.isAuto) return !q || "自动选择".includes(q) || "自动选择最优中继节点".includes(q);
    const info = ipInfoMap.value[entry.address];
    const ipMatch = info ? `${info.region} ${info.isp}`.toLowerCase().includes(q) : false;
    return !q || entry.name.toLowerCase().includes(q) || entry.address.toLowerCase().includes(q) || ipMatch;
  };
  const entries: RelayEntry[] = [autoEntry, ...relayList.value.map(r => ({ ...r, isAuto: false }))];
  return entries.filter(matches);
});

function extractHost(addr: string): string {
  const idx = addr.lastIndexOf(':');
  return idx > 0 ? addr.slice(0, idx) : addr;
}

async function fetchIpInfo(addr: string) {
  if (ipInfoMap.value[addr]) return;
  const host = extractHost(addr);
  try {
    const info = await invoke<IpInfo>("get_ip_info", { host });
    ipInfoMap.value = { ...ipInfoMap.value, [addr]: info };
  } catch (e: any) {
    props.showToast(`查询 ${host} 节点地域失败：${e}`);
    ipInfoMap.value = { ...ipInfoMap.value, [addr]: { region: "", isp: "" } };
  }
}

watch(searchQuery, (val) => {
  if (pingTimer) clearTimeout(pingTimer);
  customRelay.value = null;

  const hasAddr = /[.:]/.test(val);
  if (!hasAddr) return;

  customRelay.value = { address: val, latency: 0, pinging: true };
  pingTimer = setTimeout(async () => {
    try {
      const ms = await invoke<number>("ping_relay", { address: val });
      customRelay.value = { address: val, latency: ms, pinging: false };
      fetchIpInfo(val);
    } catch {
      customRelay.value = null;
    }
  }, 500);
});

onMounted(async () => {
  try {
    const relays = await invoke<RelayInfo[]>("get_relays");
    relayList.value = relays;
    for (const r of relays) {
      fetchIpInfo(r.address);
    }
  } catch (e) {
    console.error("获取中继列表失败", e);
  }
});
</script>

<template>
  <div>
    <div class="relay-search-wrap">
      <i class="bi bi-search"></i>
      <input v-model="searchQuery" class="relay-search" placeholder="搜索或输入节点地址" />
    </div>

    <div class="relay-list">
      <div
        v-for="entry in filteredEntries"
        :key="entry.id"
        class="relay-card"
        :class="{ selected: selectedRelay === entry.id }"
        @click="selectedRelay = entry.id"
      >
        <div class="relay-card-left">
          <i v-if="entry.isAuto" class="bi bi-router"></i>
          <i v-else class="bi bi-hdd-network"></i>
          <div>
            <span class="relay-name">{{ entry.name }}</span>
            <span class="relay-addr">
              {{ entry.address }}
              <span v-if="ipInfoMap[entry.address]?.region" class="relay-ipinfo">
                {{ ipInfoMap[entry.address].region }} · {{ ipInfoMap[entry.address].isp }}
              </span>
              <span v-else-if="ipInfoMap[entry.address]" class="relay-ipinfo unknown">未知</span>
            </span>
          </div>
        </div>
        <i v-if="selectedRelay === entry.id" class="bi bi-check-circle-fill check-icon"></i>
      </div>

      <div
        v-if="customRelay && !customRelay.pinging"
        class="relay-card"
        :class="{ selected: selectedRelay === customRelay.address }"
        @click="selectedRelay = customRelay!.address"
      >
        <div class="relay-card-left">
          <i class="bi bi-plug"></i>
          <div>
            <span class="relay-name">{{ customRelay.address }}</span>
            <span class="relay-addr">
              {{ customRelay.latency }}ms
              <span v-if="ipInfoMap[customRelay.address]?.region" class="relay-ipinfo">
                {{ ipInfoMap[customRelay.address].region }} · {{ ipInfoMap[customRelay.address].isp }}
              </span>
              <span v-else-if="ipInfoMap[customRelay.address]" class="relay-ipinfo unknown">未知</span>
            </span>
          </div>
        </div>
        <i v-if="selectedRelay === customRelay.address" class="bi bi-check-circle-fill check-icon"></i>
      </div>

      <div v-if="customRelay && customRelay.pinging" class="text-muted" style="padding:20px 0;text-align:center">
        正在探测...
      </div>

      <div v-if="filteredEntries.length === 0 && !customRelay" class="text-muted" style="padding:30px 0;text-align:center">
        暂无匹配的中继服务器
      </div>
    </div>
  </div>
</template>

<style scoped>
.relay-list {
  display: flex;
  flex-direction: column;
  gap: 10px;
  margin-top: 16px;
}

.relay-card {
  background: var(--bg-card);
  border: 1px solid var(--border-color);
  border-radius: 10px;
  padding: 14px 16px;
  cursor: pointer;
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.15);
  transition: all 0.2s ease;
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.relay-card:hover {
  border-color: var(--accent-primary);
  background: rgba(255, 255, 255, 0.05);
}

.relay-card-left {
  display: flex;
  align-items: center;
  gap: 14px;
  font-size: 22px;
  color: var(--accent-primary);
}

.relay-card-left div {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.relay-name {
  color: var(--text-primary);
  font-size: 14px;
  font-weight: 500;
}

.relay-addr {
  color: var(--text-muted);
  font-size: 12px;
}

.relay-ipinfo {
  color: var(--accent-primary);
  opacity: 0.8;
  font-size: 11px;
  margin-left: 4px;
}

.relay-ipinfo.unknown {
  color: var(--text-muted);
  opacity: 0.6;
}

.relay-search-wrap {
  position: relative;
  display: flex;
  align-items: center;
  background: rgba(255, 255, 255, 0.08);
  border-radius: 8px;
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.15);
  transition: all 0.2s ease;
}

.relay-search-wrap:focus-within {
  background: rgba(255, 255, 255, 0.12);
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.2);
}

.relay-search-wrap i:first-child {
  position: absolute;
  left: 12px;
  font-size: 14px;
  color: var(--text-muted);
  pointer-events: none;
  z-index: 1;
}

.relay-search {
  width: 100%;
  padding: 12px 14px 12px 34px;
  border: none;
  border-radius: 8px;
  font-size: 13px;
  box-sizing: border-box;
  background: transparent;
  color: var(--text-primary);
  outline: none;
}

.relay-search::placeholder {
  color: var(--text-muted);
}

.relay-card.selected {
  border-color: var(--accent-primary);
  background: rgba(0, 102, 204, 0.08);
  box-shadow: 0 2px 12px rgba(0, 102, 204, 0.2);
}

.check-icon {
  font-size: 18px;
  color: var(--accent-primary);
  flex-shrink: 0;
}
</style>