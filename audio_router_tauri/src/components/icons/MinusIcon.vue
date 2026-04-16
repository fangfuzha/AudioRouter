<script setup lang="ts">
interface Props {
  width?: number;
  height?: number;
  strokeWidth?: number;
}

withDefaults(defineProps<Props>(), {
  width: 20,
  height: 20,
  strokeWidth: 2,
});
</script>

<template>
  <svg
    :width="width"
    :height="height"
    xmlns="http://www.w3.org/2000/svg"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    :stroke-width="strokeWidth"
    stroke-linecap="round"
    stroke-linejoin="round"
  >
    <line x1="5" y1="12" x2="19" y2="12"></line>
  </svg>
</template>
