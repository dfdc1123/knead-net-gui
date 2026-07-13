<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { open } from "@tauri-apps/plugin-dialog";

  type FolderEntry = { name: string; path: string; ext: string; bytes: number };

  let folder = $state<string | null>(null);
  let entries = $state<FolderEntry[]>([]);
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
    try {
      folder = path;
      entries = await invoke<FolderEntry[]>("list_folder", { path });

      // 找 .pcb → 存后端 (供 Step 3 布局用)
      const pcb = entries.find((e) => e.ext === "kicad_pcb");
      if (pcb) {
        await invoke("set_pcb_path", { path: pcb.path });
      }

      // 找 .sch → 渲染 SVG
      const sch = entries.find((e) => e.ext === "kicad_sch");
      if (sch) {
        svg = await invoke<string>("render_sch", { path: sch.path });
      } else {
        svg = "";
      }
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  function formatBytes(n: number) {
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
    return `${(n / 1024 / 1024).toFixed(2)} MB`;
  }
</script>

<div class="h-full flex flex-col gap-4 p-6 overflow-auto">
  <!-- 顶部:选择按钮 + 当前路径 -->
  <div class="flex items-center gap-3">
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
    <div class="alert alert-error text-sm">{error}</div>
  {/if}

  {#if entries.length > 0}
    <div class="grid grid-cols-[280px_1fr] gap-4 flex-1 min-h-0">
      <!-- 左:文件列表 -->
      <div class="card bg-base-200 overflow-auto">
        <div class="card-body p-3">
          <h3 class="text-xs font-semibold uppercase tracking-wider text-base-content/50 px-1 pb-2">
            文件 ({entries.length})
          </h3>
          <ul class="menu menu-sm w-full p-0">
            {#each entries as e}
              <li>
                <div class="flex justify-between items-center gap-2">
                  <span class="truncate font-mono text-xs">{e.name}</span>
                  <span class="badge badge-ghost badge-sm shrink-0">{formatBytes(e.bytes)}</span>
                </div>
              </li>
            {/each}
          </ul>
        </div>
      </div>

      <!-- 右:原理图预览 -->
      <div class="card bg-base-200 overflow-auto">
        <div class="card-body p-3">
          <h3 class="text-xs font-semibold uppercase tracking-wider text-base-content/50 px-1 pb-2">
            原理图预览
          </h3>
          {#if svg}
                      <div class="bg-white rounded p-2 flex items-center justify-center max-h-[60vh] overflow-auto">
                        {@html svg}
                      </div>
                    {:else}
            <div class="text-sm text-base-content/40 text-center py-12">
              文件夹中没有 .kicad_sch
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