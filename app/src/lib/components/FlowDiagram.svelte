<script lang="ts">
  import type { ModeDto } from '../host/modes';
  import { layout } from '../modes/flow';
  import { NODE_H, NODE_W } from '../modes/flow';

  let {
    modes,
    defaultMode,
    selected = null,
    onselect
  }: {
    modes: Pick<ModeDto, 'id' | 'name' | 'next' | 'autoAdvanceTo'>[];
    defaultMode: string;
    selected?: string | null;
    onselect?: (id: string) => void;
  } = $props();

  const flow = $derived(layout(modes, defaultMode));
  const label = (id: string) => flow.nodes.find((n) => n.id === id)?.label ?? id;
</script>

{#if modes.length}
  <div class="flow">
    <svg
      viewBox={`0 0 ${flow.width} ${flow.height}`}
      width={flow.width}
      height={flow.height}
      role="group"
      aria-label="Mode flow diagram"
      data-testid="flow"
    >
      <defs>
        <marker
          id="arrow-next"
          viewBox="0 0 10 10"
          refX="9"
          refY="5"
          markerWidth="7"
          markerHeight="7"
          orient="auto-start-reverse"
        >
          <path d="M0,0 L10,5 L0,10 z" class="head next" />
        </marker>
        <marker
          id="arrow-auto"
          viewBox="0 0 10 10"
          refX="9"
          refY="5"
          markerWidth="6"
          markerHeight="6"
          orient="auto-start-reverse"
        >
          <path d="M0,0 L10,5 L0,10 z" class="head auto" />
        </marker>
      </defs>
      {#each flow.edges as e (`${e.from}>${e.to}`)}
        <path
          d={e.path}
          class="edge"
          class:auto={e.auto}
          fill="none"
          marker-end={e.auto ? 'url(#arrow-auto)' : 'url(#arrow-next)'}
          data-edge={`${e.from}>${e.to}`}
          data-kind={e.auto ? 'auto' : 'next'}
        />
      {/each}
      {#each flow.nodes as n (n.id)}
        <g
          class="node"
          class:sel={n.id === selected}
          transform={`translate(${n.x},${n.y})`}
          role="button"
          tabindex="0"
          aria-label={`Mode ${n.label}${n.isDefault ? ' (default)' : ''}`}
          aria-pressed={n.id === selected}
          onclick={() => onselect?.(n.id)}
          onkeydown={(e) => {
            if (e.key === 'Enter' || e.key === ' ') {
              e.preventDefault();
              onselect?.(n.id);
            }
          }}
        >
          <rect width={NODE_W} height={NODE_H} rx="8" />
          <text
            x={NODE_W / 2}
            y={n.isDefault || n.anyTarget ? 19 : 28}
            text-anchor="middle"
            class="name">{n.label.length > 18 ? `${n.label.slice(0, 17)}…` : n.label}</text
          >
          {#if n.isDefault || n.anyTarget}
            <text x={NODE_W / 2} y="35" text-anchor="middle" class="sub"
              >{[n.isDefault ? 'default' : '', n.anyTarget ? 'any next' : '']
                .filter(Boolean)
                .join(' · ')}</text
            >
          {/if}
        </g>
      {/each}
    </svg>
    <div class="legend" aria-hidden="true">
      <span
        ><svg width="34" height="10"><line x1="1" y1="5" x2="33" y2="5" class="edge" /></svg> proposes
        (asks you first)</span
      >
      <span
        ><svg width="34" height="10"><line x1="1" y1="5" x2="33" y2="5" class="edge auto" /></svg> auto-advance
        (no confirmation)</span
      >
    </div>
    <ul class="sr-only" aria-label="Transitions">
      {#each flow.edges as e (`${e.from}>${e.to}`)}
        <li>{label(e.from)} to {label(e.to)}{e.auto ? ' (auto-advance)' : ''}</li>
      {/each}
    </ul>
  </div>
{:else}
  <p class="muted">No modes.</p>
{/if}

<style>
  .flow {
    overflow-x: auto;
    padding-bottom: 4px;
  }
  svg {
    display: block;
    max-width: none;
  }
  .edge {
    stroke: var(--muted);
    stroke-width: 1.4;
  }
  .edge.auto {
    stroke: var(--accent);
    stroke-width: 2.6;
  }
  .head.next {
    fill: var(--muted);
  }
  .head.auto {
    fill: var(--accent);
  }
  .node {
    cursor: pointer;
  }
  .node rect {
    fill: var(--surface);
    stroke: var(--border);
    stroke-width: 1.2;
  }
  .node:hover rect,
  .node:focus-visible rect {
    stroke: var(--accent);
  }
  .node.sel rect {
    stroke: var(--accent);
    stroke-width: 2.4;
  }
  .node:focus-visible {
    outline: none;
  }
  .name {
    fill: var(--text);
    font-size: 13px;
    font-weight: 600;
  }
  .sub {
    fill: var(--muted);
    font-size: 10.5px;
  }
  .legend {
    display: flex;
    gap: 16px;
    flex-wrap: wrap;
    font-size: 12px;
    color: var(--muted);
    margin-top: 4px;
  }
  .legend svg {
    display: inline-block;
    vertical-align: middle;
  }
</style>
