<script setup lang="ts">
import { ref } from 'vue';

interface IconSidebarProps {
  activeIcon: string;
  iconItems: Array<{ id: string; icon: string; title: string }>;
}

defineProps<IconSidebarProps>();
const emit = defineEmits<{
  iconChange: [icon: string]
}>();

const pressedIconId = ref<string | null>(null);
</script>

<template>
  <div class="icon-sidebar">
    <div
      v-for="item in iconItems"
      :key="item.id"
      :class="['sidebar-icon-item', { active: activeIcon === item.id, 'clickable-active': pressedIconId === item.id }]"
      :title="item.title"
      @mousedown="pressedIconId = item.id"
      @mouseup="pressedIconId = null"
      @mouseleave="pressedIconId = null"
      @click="emit('iconChange', item.id)"
    >
      <i :class="['bi', item.icon]"></i>
    </div>
  </div>
</template>

<style scoped>
.icon-sidebar {
  width: 60px;
  background: rgba(30, 30, 46, 0.08);
  backdrop-filter: blur(24px);
  -webkit-backdrop-filter: blur(24px);
  display: flex;
  flex-direction: column;
  align-items: center;
  padding: 10px 0;
  gap: 8px;
  -webkit-app-region: no-drag;
  transition: background 0.3s ease;
  border-right: 1px solid rgba(255, 255, 255, 0.03);
}

@media (prefers-color-scheme: light) {
  .icon-sidebar {
    background: rgba(255, 255, 255, 0.2);
    border-right: 1px solid rgba(255, 255, 255, 0.12);
    backdrop-filter: blur(24px);
    -webkit-backdrop-filter: blur(24px);
  }
}

[data-theme="light"] .icon-sidebar {
  background: rgba(255, 255, 255, 0.2);
  border-right: 1px solid rgba(255, 255, 255, 0.12);
  backdrop-filter: blur(24px);
  -webkit-backdrop-filter: blur(24px);
}
</style>
