<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { open } from "@tauri-apps/plugin-dialog";
  import { locale, ui } from "$lib/i18n";
  import Panel from "./Panel.svelte";

  type FolderEntry = { name: string; path: string; ext: string; bytes: number };
  type Project = { name: string; sch?: FolderEntry; pcb?: FolderEntry };

  let {
    onStatusChange = () => {},
    onSchematicChange = () => {},
  }: {
    onStatusChange?: (ready: boolean) => void;
    onSchematicChange?: (svg: string) => void;
  } = $props();

  let folder = $state<string | null>(null);
  let entries = $state<FolderEntry[]>([]);
  let projects = $state<Project[]>([]);
  let selectedProject = $state<string | null>(null);
  let selectedHasSchematic = $state(false);
  let svg = $state<string>("");
  let error = $state<string>("");
  let busy = $state(false);

  async function pickFolder() {
    try {
      error = "";
      const picked = await open({ directory: true, multiple: false });
      if (typeof picked === "string") {
        await loadFolder(picked);
      }
    } catch (e) {
      error = String(e);
    }
  }

  async function loadFolder(path: string) {
    busy = true;
    error = "";
    folder = path;
    entries = [];
    projects = [];
    selectedProject = null;
    selectedHasSchematic = false;
    svg = "";
    onSchematicChange("");
    onStatusChange(false);
    try {
      await invoke("clear_project_source");
      const nextEntries = await invoke<FolderEntry[]>("list_folder", { path, locale });
      const projectsByStem = new Map<string, Project>();
      for (const entry of nextEntries) {
        if (entry.ext !== "kicad_sch" && entry.ext !== "kicad_pcb") continue;
        const name = fileStem(entry.name);
        const project = projectsByStem.get(name) ?? { name };
        if (entry.ext === "kicad_sch") project.sch = entry;
        if (entry.ext === "kicad_pcb") project.pcb = entry;
        projectsByStem.set(name, project);
      }
      const nextProjects = [...projectsByStem.values()];

      entries = nextEntries;
      projects = nextProjects;

      if (nextProjects.length > 0) {
        await selectProject(nextProjects.find((project) => project.pcb) ?? nextProjects[0]);
      }
    } catch (e) {
      error = String(e);
      entries = [];
      projects = [];
      selectedProject = null;
      selectedHasSchematic = false;
      svg = "";
    } finally {
      busy = false;
    }
  }

  async function selectProject(project: Project) {
    busy = true;
    error = "";
    selectedProject = project.name;
    selectedHasSchematic = Boolean(project.sch);
    svg = "";
    onSchematicChange("");
    onStatusChange(false);

    try {
      await invoke("clear_project_source");
      if (project.sch) {
        svg = await invoke<string>("render_sch", {
          path: project.sch.path,
          pcbPath: project.pcb?.path ?? null,
          locale,
        });
        onSchematicChange(svg);
      }
      if (!project.pcb) {
        error = ui.step1.matchingPcbMissing(project.name);
        return;
      }
      await invoke("set_pcb_path", { path: project.pcb.path });
      onStatusChange(true);
    } catch (e) {
      error = String(e);
      await invoke("clear_project_source").catch(() => {});
    } finally {
      busy = false;
    }
  }

  function fileStem(name: string) {
    const dot = name.lastIndexOf(".");
    return dot === -1 ? name : name.slice(0, dot);
  }

  function formatBytes(n: number) {
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
    return `${(n / 1024 / 1024).toFixed(2)} MB`;
  }
</script>

<div class="mx-auto flex h-full min-h-0 w-full max-w-[1920px] flex-col gap-4 overflow-hidden p-6">
  <header class="shrink-0">
    <h1 class="text-2xl font-bold">{ui.step1.title}</h1>
    <p class="text-sm text-base-content/60">{ui.step1.subtitle}</p>
  </header>

  {#if error}
    <div class="alert alert-error shrink-0 text-sm" role="alert"><span>{error}</span></div>
  {/if}

  <div class="grid min-h-0 flex-1 grid-cols-[20rem_minmax(0,1fr)] gap-4">
    <Panel as="aside" class="min-w-0 overflow-y-auto">
      <div class="card-body min-h-0 min-w-0 gap-3 p-4">
        <fieldset class="fieldset min-w-0 shrink-0">
          <legend class="fieldset-legend">{ui.step1.projectFolder}</legend>
          <button
            type="button"
            class="btn btn-primary btn-sm btn-block"
            disabled={busy}
            onclick={() => void pickFolder()}
          >
            {folder ? ui.step1.changeFolder : ui.step1.chooseFolder}
          </button>
          {#if busy}
            <p class="label"><span class="loading loading-spinner loading-xs"></span>{ui.step1.scanning}</p>
          {:else if folder}
            <div class="alert alert-success alert-soft mt-1 min-w-0 overflow-hidden px-3 py-2" role="status">
              <span class="block min-w-0 max-w-full truncate font-mono text-xs" title={folder}>{folder}</span>
            </div>
          {:else}
            <p class="label text-base-content/50">{ui.step1.noFolder}</p>
          {/if}
        </fieldset>

        <div class="divider my-0"></div>

        <div class="flex min-h-0 flex-1 flex-col gap-2">
          <div class="flex items-center justify-between">
            <h2 class="card-title text-sm">{ui.step1.projects}</h2>
            {#if projects.length}<span class="badge badge-outline badge-sm">{projects.length}</span>{/if}
          </div>
          {#if projects.length > 0}
            <ul class="menu menu-sm min-h-0 w-full flex-1 overflow-auto rounded-box bg-base-200 p-2">
              {#each projects as project}
                <li>
                  <button class:menu-active={selectedProject === project.name} onclick={() => selectProject(project)} disabled={busy}>
                    <span class="min-w-0 flex-1 truncate font-mono text-xs">{project.name}</span>
                    {#if !project.pcb}
                      <span class="badge badge-error badge-xs">{ui.step1.missingPcb}</span>
                    {:else if !project.sch}
                      <span class="badge badge-ghost badge-xs">PCB</span>
                    {/if}
                  </button>
                </li>
              {/each}
            </ul>
          {:else}
            <div class="hero min-h-28 flex-1 rounded-box bg-base-200">
              <span class="text-sm text-base-content/50">{ui.step1.noFolder}</span>
            </div>
          {/if}
        </div>

        {#if entries.length > 0}
          <div class="collapse collapse-arrow shrink-0 border border-base-300 bg-base-200">
            <input type="checkbox" />
            <div class="collapse-title min-h-0 py-3 text-sm font-medium">{ui.step1.files(entries.length)}</div>
            <div class="collapse-content max-h-40 overflow-auto">
              <ul class="list">
                {#each entries as e}
                  <li class="flex items-center justify-between gap-2 py-1">
                    <span class="min-w-0 truncate font-mono text-xs">{e.name}</span>
                    <span class="badge badge-ghost badge-xs shrink-0">{formatBytes(e.bytes)}</span>
                  </li>
                {/each}
              </ul>
            </div>
          </div>
        {/if}
      </div>
    </Panel>

    <Panel>
      <div class="card-body min-h-0 gap-3 p-4">
        <div class="flex shrink-0 items-center justify-between">
          <h2 class="card-title text-sm">{ui.common.schematic}</h2>
          {#if selectedProject}<span class="badge badge-ghost badge-sm font-mono">{selectedProject}</span>{/if}
        </div>
        {#if svg}
          <div class="min-h-0 flex-1 overflow-auto rounded-box border border-base-300 bg-base-100 p-3" data-theme="nord">
            {@html svg}
          </div>
        {:else}
          <div class="hero min-h-0 flex-1 rounded-box bg-base-200">
            <span class="text-sm text-base-content/50">
              {selectedProject && selectedHasSchematic ? ui.step1.loadFailed : selectedProject ? ui.common.noSchematic : ui.step1.noPreview}
            </span>
          </div>
        {/if}
      </div>
    </Panel>
  </div>
</div>
