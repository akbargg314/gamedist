<script lang="ts">
  import { onMount } from 'svelte';
  import Sidebar from './components/Sidebar.svelte';
  import LibraryGrid from './components/LibraryGrid.svelte';
  import GamePage from './components/GamePage.svelte';
  import Discover from './components/Discover.svelte';
  import Downloads from './components/Downloads.svelte';
  import Settings from './components/Settings.svelte';
  import AddGameDialog from './components/AddGameDialog.svelte';
  import KeyChangeDialog from './components/KeyChangeDialog.svelte';
  import Toasts from './components/Toasts.svelte';
  import { app, type View } from './lib/state.svelte';
  import { isTauri } from './lib/api';

  onMount(() => {
    void app.init();
  });

  const tabs: { id: View; label: string }[] = [
    { id: 'library', label: 'Library' },
    { id: 'discover', label: 'Discover' },
    { id: 'downloads', label: 'Downloads' },
    { id: 'settings', label: 'Settings' },
  ];

  let main: HTMLElement;
  function go(view: View) {
    app.view = view;
    if (view === 'library') app.selectedId = null;
    main?.focus();
  }

  // Scroll to top on navigation.
  $effect(() => {
    void app.view;
    void app.selectedId;
    main?.scrollTo?.({ top: 0 });
  });
</script>

<a class="skip" href="#main">Skip to content</a>
<div class="shell">
  <header class="topbar">
    <div class="brand" aria-label="GameDist">
      <span class="logo" aria-hidden="true">◆</span>
      <span>GameDist</span>
      {#if !isTauri}<span class="demo" title="Running in a browser with sample data">demo</span>{/if}
    </div>
    <nav aria-label="Main">
      <ul>
        {#each tabs as t (t.id)}
          <li>
            <button
              type="button"
              class="tab"
              class:active={app.view === t.id}
              aria-current={app.view === t.id ? 'page' : undefined}
              onclick={() => go(t.id)}
            >
              {t.label}
              {#if t.id === 'downloads' && app.activeOps.length}
                <span class="count" aria-label="{app.activeOps.length} active">{app.activeOps.length}</span>
              {/if}
            </button>
          </li>
        {/each}
      </ul>
    </nav>
  </header>

  <Sidebar />

  <main id="main" bind:this={main} tabindex="-1">
    {#if app.view === 'library'}
      {#if app.selected}
        {#key app.selected.game_id}
          <GamePage game={app.selected} />
        {/key}
      {:else}
        <LibraryGrid />
      {/if}
    {:else if app.view === 'discover'}
      <Discover />
    {:else if app.view === 'downloads'}
      <Downloads />
    {:else}
      <Settings />
    {/if}
  </main>
</div>

{#if app.addDialog.open}
  {#key app.addDialog.seq}
    <AddGameDialog initialUrl={app.addDialog.url} />
  {/key}
{/if}

{#if app.keyPrompt}
  <KeyChangeDialog prompt={app.keyPrompt} />
{/if}

<Toasts />

<style>
  .shell {
    display: grid;
    grid-template-columns: 260px minmax(0, 1fr);
    grid-template-rows: auto minmax(0, 1fr);
    grid-template-areas:
      'top top'
      'side main';
    height: 100vh;
    height: 100dvh;
  }
  @media (max-width: 900px) {
    .shell {
      grid-template-columns: 210px minmax(0, 1fr);
    }
  }
  .topbar {
    grid-area: top;
    display: flex;
    align-items: center;
    gap: 24px;
    padding: 0 16px;
    height: 52px;
    background: var(--surface-0);
    border-bottom: 1px solid var(--border);
    min-width: 0;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 8px;
    font-weight: 800;
    letter-spacing: 0.02em;
    white-space: nowrap;
  }
  .logo {
    color: var(--accent);
    font-size: 1.2rem;
  }
  .demo {
    font-size: 0.65rem;
    font-weight: 700;
    text-transform: uppercase;
    padding: 1px 6px;
    border-radius: 4px;
    background: var(--warning-soft);
    color: var(--warning);
  }
  nav {
    min-width: 0;
    overflow-x: auto;
  }
  nav ul {
    display: flex;
    gap: 4px;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .tab {
    position: relative;
    display: flex;
    align-items: center;
    gap: 6px;
    height: 36px;
    padding: 0 12px;
    border: 0;
    border-radius: var(--radius);
    background: none;
    color: var(--text-dim);
    font: inherit;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    font-size: 0.85rem;
    cursor: pointer;
    white-space: nowrap;
  }
  .tab:hover {
    color: var(--text);
    background: var(--surface-2);
  }
  .tab.active {
    color: var(--text-strong);
    box-shadow: inset 0 -2px 0 var(--accent);
  }
  .count {
    min-width: 18px;
    padding: 0 5px;
    border-radius: 99px;
    background: var(--accent);
    color: var(--on-accent);
    font-size: 0.7rem;
    line-height: 18px;
    text-align: center;
  }
  .shell > :global(.sidebar) {
    grid-area: side;
  }
  main {
    grid-area: main;
    --pad: 28px;
    padding: var(--pad);
    overflow-y: auto;
    overflow-x: hidden;
    outline: none;
    min-width: 0;
  }
  @media (max-width: 900px) {
    main {
      --pad: 18px;
    }
  }
  .skip {
    position: absolute;
    left: 8px;
    top: -40px;
    z-index: 300;
    padding: 6px 10px;
    background: var(--accent);
    color: var(--on-accent);
    border-radius: var(--radius);
  }
  .skip:focus {
    top: 8px;
  }
</style>
