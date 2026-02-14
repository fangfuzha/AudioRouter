<script setup lang="ts">
interface Props {
  width?: number;
  height?: number;
}

withDefaults(defineProps<Props>(), {
  width: 16,
  height: 16,
});
</script>

<template>
  <svg
    :width="width"
    :height="height"
    xmlns="http://www.w3.org/2000/svg"
    viewBox="0 0 24 24"
    fill="currentColor"
    stroke="none"
  >
    <rect x="6" y="6" width="12" height="12"></rect>
  </svg>
</template>
