<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { open } from "@tauri-apps/plugin-dialog";

  type FolderEntry = { name: string; path: string; ext: string; bytes: number };
  type Project = { name: string; sch?: FolderEntry; pcb?: FolderEntry };

  let { onStatusChange = () => {} }: { onStatusChange?: (ready: boolean) => void } = $props();

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
    onStatusChange(false);
    try {
      await invoke("clear_pcb_path");
      const nextEntries = await invoke<FolderEntry[]>("list_folder", { path });
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
    onStatusChange(false);

    try {
      await invoke("clear_pcb_path");
      if (project.sch) {
        svg = await invoke<string>("render_sch", { path: project.sch.path });
      }
      if (!project.pcb) {
        error = `找不到与 ${project.name}.kicad_sch 同名的 .kicad_pcb 文件`;
        return;
      }
      await invoke("set_pcb_path", { path: project.pcb.path });
      onStatusChange(true);
    } catch (e) {
      error = String(e);
      await invoke("clear_pcb_path").catch(() => {});
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

<div class="h-full flex flex-col gap-4 p-6 overflow-hidden">
  <div class="flex items-center gap-3 shrink-0">
    <button class="btn btn-primary" onclick={pickFolder} disabled={busy}>
      {busy ? "加载中…" : "选择 KiCad 文件夹"}
    </button>
    {#if folder}
      <span class="text-sm text-base-content/60 font-mono truncate flex-1">{folder}</span>
    {:else}
      <span class="text-sm text-base-content/40">未选择</span>
    {/if}
  </div>

  {#if error}
    <div class="alert alert-error text-sm shrink-0">{error}</div>
  {/if}

  {#if entries.length > 0}
    <div class="grid grid-cols-[280px_1fr] gap-4 flex-1 min-h-0">
      <div class="card bg-base-200 overflow-auto">
        <div class="card-body p-3">
          <h3 class="text-xs font-semibold uppercase tracking-wider text-base-content/50 px-1 pb-2">
            KiCad 工程 ({projects.length})
          </h3>
          {#if projects.length > 0}
            <ul class="menu menu-sm w-full p-0">
              {#each projects as project}
                <li>
                  <button
                    class:menu-active={selectedProject === project.name}
                    onclick={() => selectProject(project)}
                    disabled={busy}
                  >
                    <span class="truncate font-mono text-xs">{project.name}</span>
                    {#if project.pcb && project.sch}
                      <span class="badge badge-success badge-sm shrink-0">已配对</span>
                    {:else if project.pcb}
                      <span class="badge badge-info badge-sm shrink-0">仅 PCB</span>
                    {:else}
                      <span class="badge badge-warning badge-sm shrink-0">缺 PCB</span>
                    {/if}
                  </button>
                </li>
              {/each}
            </ul>
          {:else}
            <p class="px-1 text-sm text-base-content/50">没有 KiCad 原理图或 PCB 文件</p>
          {/if}

          <div class="divider my-2"></div>
          <details>
            <summary class="cursor-pointer px-1 text-xs text-base-content/50">全部文件 ({entries.length})</summary>
            <ul class="menu menu-sm w-full p-0 mt-2">
              {#each entries as e}
              <li>
                <div class="flex justify-between items-center gap-2">
                  <span class="truncate font-mono text-xs">{e.name}</span>
                  <span class="badge badge-ghost badge-sm shrink-0">{formatBytes(e.bytes)}</span>
                </div>
              </li>
              {/each}
            </ul>
          </details>
        </div>
      </div>

      <div class="card bg-base-200 overflow-hidden">
        <div class="card-body p-3 h-full">
          <h3 class="text-xs font-semibold uppercase tracking-wider text-base-content/50 px-1 pb-2 shrink-0">
            原理图预览
          </h3>
          {#if svg}
            <!-- SVG 用 width/height:100% 撑满容器, preserveAspectRatio 自动按比例缩放并居中 -->
            <div class="flex-1 min-h-0 bg-white rounded p-2">
              {@html svg}
            </div>
          {:else if selectedProject && selectedHasSchematic}
            <div class="flex-1 flex items-center justify-center text-base-content/40 text-sm">
              原理图加载失败
            </div>
          {:else if selectedProject}
            <div class="flex-1 flex items-center justify-center text-base-content/40 text-sm">
              该工程没有同名的 .kicad_sch，仍可使用 PCB 继续
            </div>
          {:else}
            <div class="flex-1 flex items-center justify-center text-base-content/40 text-sm">
              请选择一个 KiCad 工程
            </div>
          {/if}
        </div>
      </div>
    </div>
  {:else}
    <div class="flex-1 flex items-center justify-center text-base-content/40 text-sm">
      选择一个包含 KiCad 项目的文件夹开始
    </div>
  {/if}
</div>
