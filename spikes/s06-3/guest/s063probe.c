/* s063probe: spike S06-3's helper inside the Debian guest (ADR 0006).
 * Our own code, built static in CI; never shipped.
 *
 *   s063probe mark N        OUT N to the probe port (the host logs its time)
 *   s063probe bench N       N port-I/O round trips, each timed with RDTSC
 *   s063probe time CMD...   runs CMD between markers 2 and 3
 *
 * Under our hypervisor an IN from 0x5E0 reads 0x53060003 and 0x5E8 the host's
 * TSC rate. Under plain KVM (the baseline) the ports are QEMU's unassigned
 * ones: OUTs are dropped, and the TSC rate is timed against CLOCK_MONOTONIC.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/io.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#define PROBE 0x5E0
#define PROBE_BENCH 0x5E4
#define PROBE_TSC_KHZ 0x5E8
#define MAGIC 0x53060003u

static unsigned long long rdtsc(void) {
  unsigned lo, hi;
  __asm__ volatile("lfence; rdtsc" : "=a"(lo), "=d"(hi));
  return ((unsigned long long)hi << 32) | lo;
}

static double now_ms(void) {
  struct timespec t;
  clock_gettime(CLOCK_MONOTONIC, &t);
  return t.tv_sec * 1e3 + t.tv_nsec / 1e6;
}

static int ours(void) { return inl(PROBE) == MAGIC; }

static unsigned tsc_khz(void) {
  if (ours()) return inl(PROBE_TSC_KHZ);
  double t0 = now_ms();
  unsigned long long c0 = rdtsc();
  while (now_ms() - t0 < 200) {
  }
  double t1 = now_ms();
  return (unsigned)((rdtsc() - c0) / (t1 - t0));
}

static int cmp(const void *a, const void *b) {
  unsigned long long x = *(const unsigned long long *)a, y = *(const unsigned long long *)b;
  return x < y ? -1 : x > y;
}

static int bench(long n) {
  unsigned long long *d = malloc(n * sizeof *d);
  if (!d) return 1;
  outl(0x10, PROBE);
  for (long i = 0; i < n; i++) {
    unsigned long long t = rdtsc();
    outl(0, PROBE_BENCH);
    d[i] = rdtsc() - t;
  }
  outl(0x11, PROBE);
  qsort(d, n, sizeof *d, cmp);
  unsigned khz = tsc_khz();
  double us = khz ? d[n / 2] * 1e3 / khz : 0;
  printf("s06-3 guest: pio-roundtrip median %llu cycles = %.2f us, p99 %llu cycles, over %ld (TSC %u kHz, %s)\n",
         d[n / 2], us, d[n * 99 / 100], n, khz, ours() ? "our hypervisor" : "plain KVM");
  free(d);
  return 0;
}

static int timed(char **argv) {
  outl(2, PROBE);
  double t0 = now_ms();
  pid_t pid = fork();
  if (pid == 0) {
    execvp(argv[0], argv);
    perror("s063probe: exec");
    _exit(127);
  }
  int status = 0;
  waitpid(pid, &status, 0);
  double t1 = now_ms();
  outl(3, PROBE);
  printf("s06-3 guest: %s took %.0f ms (guest clock), status %d\n", argv[0], t1 - t0, WEXITSTATUS(status));
  return 0;
}

int main(int argc, char **argv) {
  if (argc < 3) {
    fprintf(stderr, "usage: s063probe mark N | bench N | time CMD...\n");
    return 2;
  }
  if (ioperm(PROBE, 12, 1)) {
    perror("s063probe: ioperm");
    return 1;
  }
  if (!strcmp(argv[1], "mark")) {
    outl((unsigned)strtoul(argv[2], 0, 0), PROBE);
    return 0;
  }
  if (!strcmp(argv[1], "bench")) return bench(strtol(argv[2], 0, 0));
  if (!strcmp(argv[1], "time")) return timed(argv + 2);
  return 2;
}
