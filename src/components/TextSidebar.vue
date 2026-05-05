<script setup lang="ts">
import { ref, watch, onMounted } from 'vue';
import { cn } from '../lib/utils';
import Button from './ui/Button.vue';

interface TextSidebarProps {
  activeIcon: string;
  activeText: string;
}

const props = defineProps<TextSidebarProps>();
const emit = defineEmits<{
  textChange: [text: string]
}>();

const showSidebarAnimation = ref(false);
const previousIcon = ref('');

onMounted(() => {
  previousIcon.value = props.activeIcon;
  showSidebarAnimation.value = true;
});

watch(() => props.activeIcon, (newIcon, oldIcon) => {
  if (newIcon !== oldIcon) {
    previousIcon.value = newIcon;
    showSidebarAnimation.value = true;
    setTimeout(() => {
      showSidebarAnimation.value = false;
    }, 300);
  }
});
</script>

<template>
  <!-- 联机侧边栏 -->
  <div v-if="activeIcon === 'connect'" class="text-sidebar" :class="{ 'animate-in': showSidebarAnimation }">
    <Button
      variant="ghost"
      :class="cn('sidebar-text-item !justify-start text-item', { active: activeText === 'join' })"
      @click="emit('textChange', 'join')"
    >
      加入房间
    </Button>
    <Button
      variant="ghost"
      :class="cn('sidebar-text-item !justify-start text-item', { active: activeText === 'create' })"
      @click="emit('textChange', 'create')"
    >
      创建房间
    </Button>
  </div>
  
  <!-- 中继服务器侧边栏 -->
  <div v-if="activeIcon === 'relay'" class="text-sidebar" :class="{ 'animate-in': showSidebarAnimation }">
    <Button
      variant="ghost"
      :class="cn('sidebar-text-item !justify-start text-item', { active: activeText === 'server' })"
      @click="emit('textChange', 'server')"
    >
      服务器设置
    </Button>
    <Button
      variant="ghost"
      :class="cn('sidebar-text-item !justify-start text-item', { active: activeText === 'logs' })"
      @click="emit('textChange', 'logs')"
    >
      运行日志
    </Button>
  </div>
</template>

<style scoped>
.text-sidebar {
  width: 180px;
  background: rgba(30, 30, 46, 0.12);
  backdrop-filter: blur(24px);
  -webkit-backdrop-filter: blur(24px);
  padding: 15px 10px;
  display: flex;
  flex-direction: column;
  gap: 5px;
  -webkit-app-region: no-drag;
  transition: all 0.3s ease;
  border-right: 1px solid rgba(255, 255, 255, 0.03);
}

@media (prefers-color-scheme: light) {
  .text-sidebar {
    background: rgba(255, 255, 255, 0.25);
    border-right: 1px solid rgba(255, 255, 255, 0.15);
    backdrop-filter: blur(24px);
    -webkit-backdrop-filter: blur(24px);
  }
}

[data-theme="light"] .text-sidebar {
  background: rgba(255, 255, 255, 0.25);
  border-right: 1px solid rgba(255, 255, 255, 0.15);
  backdrop-filter: blur(24px);
  -webkit-backdrop-filter: blur(24px);
}

.text-item {
  opacity: 0;
  animation: itemEnter 0.4s ease-out forwards;
}

.text-item:nth-child(1) { animation-delay: 0s; }
.text-item:nth-child(2) { animation-delay: 0.05s; }
.text-item:nth-child(3) { animation-delay: 0.1s; }
.text-item:nth-child(4) { animation-delay: 0.15s; }

@keyframes itemEnter {
  0% {
    opacity: 0;
    transform: translateX(-15px) translateY(10px);
  }
  100% {
    opacity: 1;
    transform: translateX(0) translateY(0);
  }
}
</style>
