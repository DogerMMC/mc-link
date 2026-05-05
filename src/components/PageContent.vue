<script setup lang="ts">
defineProps({
  activeIcon: String,
  activeText: String,
});
</script>

<template>
  <div class="center-content">
    <slot />
  </div>
</template>

<style scoped>
.center-content {
  flex: 1;
  overflow: hidden;
  background: #1e1e2e;
}

@media (prefers-color-scheme: light) {
  .center-content {
    background: #f8fafc;
  }
}

[data-theme="light"] .center-content {
  background: #f8fafc;
}
</style>
