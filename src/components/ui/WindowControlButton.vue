<script setup lang="ts">
import { ref } from 'vue';

interface Props {
  variant: 'minimize' | 'maximize' | 'close';
  title?: string;
}

defineProps<Props>();
const emit = defineEmits<{
  click: [];
}>();

const isPressed = ref(false);

const handleClick = () => {
  emit('click');
};

const iconMap = {
  minimize: 'bi-dash-lg',
  maximize: 'bi-fullscreen',
  close: 'bi-x-lg',
};

const colorMap = {
  minimize: '#ffbd2e',
  maximize: '#27c93f',
  close: '#ff5f56',
};
</script>

<template>
  <button
    class="window-control-btn"
    :class="[variant, { pressed: isPressed }]"
    :title="title"
    @mousedown="isPressed = true"
    @mouseup="isPressed = false"
    @mouseleave="isPressed = false"
    @click="handleClick"
  >
    <i :class="['bi', iconMap[variant]]"></i>
  </button>
</template>

<style scoped>
.window-control-btn {
  width: 14px;
  height: 14px;
  border-radius: 50%;
  border: none;
  cursor: pointer;
  transition: all 0.2s ease;
  display: flex;
  align-items: center;
  justify-content: center;
  position: relative;
  background: v-bind('colorMap[variant]');
}

.window-control-btn:hover {
  transform: scale(1.15);
  filter: brightness(1.1);
}

.window-control-btn.pressed {
  transform: scale(0.9);
}

.window-control-btn i {
  font-size: 8px;
  color: rgba(0, 0, 0, 0.5);
  opacity: 0;
  transition: opacity 0.2s ease;
}

.window-control-btn:hover i {
  opacity: 1;
}
</style>
