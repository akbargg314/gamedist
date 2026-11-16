<script lang="ts">
  import { hueOf } from '../lib/format';

  interface Props {
    src: string | null;
    title: string;
    kind?: 'icon' | 'banner' | 'card';
    alt?: string;
  }

  let { src, title, kind = 'icon', alt = '' }: Props = $props();

  let failed = $state(false);
  let loaded = $state(false);
  $effect(() => {
    // Reset when the source changes.
    void src;
    failed = false;
    loaded = false;
  });

  const hue = $derived(hueOf(title));
  const initial = $derived((title.trim()[0] ?? '?').toUpperCase());
</script>

<div class="img {kind}" style:--hue={hue} aria-hidden={alt ? undefined : 'true'}>
  {#if !loaded}
    <span class="ph">{initial}</span>
  {/if}
  {#if src && !failed}
    <img
      {src}
      {alt}
      class:show={loaded}
      loading="lazy"
      decoding="async"
      referrerpolicy="no-referrer"
      draggable="false"
      onload={() => (loaded = true)}
      onerror={() => (failed = true)}
    />
  {/if}
</div>

<style>
  .img {
    position: relative;
    overflow: hidden;
    display: grid;
    place-items: center;
    background:
      radial-gradient(circle at 25% 20%, hsl(var(--hue) 70% 62% / 0.55), transparent 60%),
      linear-gradient(135deg, hsl(var(--hue) 45% 30%), hsl(calc(var(--hue) + 50) 50% 18%));
    color: hsl(var(--hue) 80% 92%);
    width: 100%;
    height: 100%;
  }
  .icon {
    border-radius: var(--radius);
  }
  .ph {
    font-weight: 800;
    font-size: 1.1em;
    line-height: 1;
    text-shadow: 0 2px 8px rgb(0 0 0 / 0.35);
    user-select: none;
  }
  .banner .ph {
    font-size: clamp(3rem, 10vw, 7rem);
    opacity: 0.35;
  }
  .card .ph {
    font-size: 3rem;
    opacity: 0.8;
  }
  img {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
    opacity: 0;
    transition: opacity 0.2s;
  }
  img.show {
    opacity: 1;
  }
</style>
