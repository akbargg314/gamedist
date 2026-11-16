<script lang="ts">
  import { untrack } from 'svelte';
  import { pickDirectory } from '../lib/api';
  import type { LibraryEntry } from '../lib/types';
  import { app } from '../lib/state.svelte';
  import { defaultDir } from '../lib/format';

  interface Props {
    game: LibraryEntry;
    dir: string;
    channel: string;
  }

  let { game, dir = $bindable(), channel = $bindable() }: Props = $props();

  // Fill defaults once, when the form appears.
  untrack(() => {
    if (!dir) dir = game.installed?.dir ?? defaultDir(app.settings?.install_root, game.slug);
    if (!channel) channel = game.channel || 'stable';
  });

  const channels = $derived(Object.entries(game.channels ?? {}));
  const uid = Math.random().toString(36).slice(2);

  async function browse() {
    const picked = await pickDirectory(dir || app.settings?.install_root, `Install ${game.title} to…`);
    if (picked) dir = picked;
  }
</script>

<div class="form">
  <label class="field" for="dir-{uid}">
    <span>Install folder</span>
    <div class="row">
      <input id="dir-{uid}" class="input mono" type="text" bind:value={dir} spellcheck="false" autocomplete="off" />
      <button type="button" class="btn" onclick={browse}>Browse…</button>
    </div>
    <small>The folder must be empty or an existing install of this game.</small>
  </label>
  {#if channels.length > 1}
    <label class="field" for="ch-{uid}">
      <span>Channel</span>
      <select id="ch-{uid}" class="input" bind:value={channel}>
        {#each channels as [name, version] (name)}
          <option value={name}>{name} — {version}</option>
        {/each}
      </select>
    </label>
  {/if}
</div>

<style>
  .form {
    display: grid;
    gap: 14px;
  }
  .row {
    display: flex;
    gap: 8px;
  }
  .row input {
    flex: 1;
    min-width: 0;
  }
</style>
