/* The OS's actions (ADR 0012 D6), registry version 1. Each returns a
   status: a negative KAI_E* on failure. */
#define KAI_EREFUSED (-1)
#define KAI_EFAILED (-5)
#define KAI_E2BIG (-7)
#define KAI_EFAULT (-14)
#define KAI_EINVAL (-22)
#define KAI_NONE (-9223372036854775807LL - 1)
/* number of CPUs online */
__KAI_IMPORT("cpus") I64 cpus(I64 *out);
/* free and total memory in MiB, and the kernel heap in use in KiB */
struct kai_memory { I64 free_mib; I64 total_mib; I64 heap_kib; };
__KAI_IMPORT("memory") I64 memory(struct kai_memory *out);
/* milliseconds since boot (1000 ms is 1 s) */
__KAI_IMPORT("uptime_ms") I64 uptime_ms(I64 *out);
/* the running services */
struct kai_processes_item { I64 pid; char name[65]; };
__KAI_IMPORT("processes") I64 processes(struct kai_processes_item *out, I64 cap);
/* scheduler counters since boot */
struct kai_threads { I64 switches; I64 ejections; I64 migrations; };
__KAI_IMPORT("threads") I64 threads(struct kai_threads *out);
/* the scheduling policy, "interactive" or "round-robin" */
__KAI_IMPORT("get_policy") I64 get_policy(char *out, I64 cap);
/* switches every CPU to "interactive" or "round-robin" */
__KAI_IMPORT("set_policy") I64 set_policy(const char *name);
/* stops or kills processes by name */
__KAI_IMPORT("process_stop") I64 process_stop(const char *name);
/* stays stopped until started */
__KAI_IMPORT("service_stop") I64 service_stop(const char *name);
/* starts a stopped service */
__KAI_IMPORT("service_start") I64 service_start(const char *name);
/* loses what it held */
__KAI_IMPORT("service_restart") I64 service_restart(const char *name);
/* installs a package; undo puts the old set back */
__KAI_IMPORT("pkg_install") I64 pkg_install(const char *name);
/* removes a package */
__KAI_IMPORT("pkg_remove") I64 pkg_remove(const char *name);
/* installed packages */
__KAI_IMPORT("pkg_list") I64 pkg_list(char *out, I64 cap);
/* packages in the repository */
__KAI_IMPORT("pkg_search") I64 pkg_search(const char *text, char *out, I64 cap);
/* a folder's names */
__KAI_IMPORT("fs_list") I64 fs_list(const char *path, char *out, I64 cap);
/* a file's text */
__KAI_IMPORT("fs_read") I64 fs_read(const char *path, char *out, I64 cap);
/* a file's earlier versions */
__KAI_IMPORT("fs_history") I64 fs_history(const char *path, char *out, I64 cap);
/* files changed today */
__KAI_IMPORT("fs_changes") I64 fs_changes(char *out, I64 cap);
/* a new file; never overwrites */
__KAI_IMPORT("fs_create") I64 fs_create(const char *path, const char *text);
/* a file's text, replaced */
__KAI_IMPORT("fs_write") I64 fs_write(const char *path, const char *text);
/* moves or renames */
__KAI_IMPORT("fs_move") I64 fs_move(const char *from, const char *to);
/* to the trash; undo brings it back */
__KAI_IMPORT("fs_trash") I64 fs_trash(const char *path);
/* an earlier version, or out of the trash */
__KAI_IMPORT("fs_restore") I64 fs_restore(const char *path);
/* no undo */
__KAI_IMPORT("fs_empty_trash") I64 fs_empty_trash(void);
