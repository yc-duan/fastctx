// Runs a program as an older Linux would: every x86_64 system call numbered FIRST_MISSING or above
// answers ENOSYS, for the program and everything it starts. 314 leaves the calls upstream Linux
// 3.13 had; distribution backports can differ. Arguments missing on 3.10 are refused too: a
// timer descriptor on the boot-time clock (3.15) or alarm clocks (3.11).
//   oldkernel FIRST_MISSING PROGRAM [ARGS...]
#define _GNU_SOURCE
#include <errno.h>
#include <linux/audit.h>
#include <linux/filter.h>
#include <linux/seccomp.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/prctl.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc < 3) {
        fprintf(stderr, "usage: %s FIRST_MISSING PROGRAM [ARGS...]\n", argv[0]);
        return 2;
    }
    unsigned first = (unsigned)strtoul(argv[1], NULL, 10);
    struct sock_filter filter[] = {
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, arch)),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, AUDIT_ARCH_X86_64, 1, 0),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, nr)),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_timerfd_create, 0, 5),
        // The clock, the low word of the first argument on a little-endian machine.
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, args[0])),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, CLOCK_BOOTTIME, 2, 0),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, CLOCK_REALTIME_ALARM, 1, 0),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, CLOCK_BOOTTIME_ALARM, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | (EINVAL & SECCOMP_RET_DATA)),
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, nr)),
        BPF_JUMP(BPF_JMP | BPF_JGE | BPF_K, first, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | (ENOSYS & SECCOMP_RET_DATA)),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    struct sock_fprog program = {.len = sizeof filter / sizeof filter[0], .filter = filter};
    if (prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) || prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &program)) {
        perror("seccomp");
        return 1;
    }
#if defined(__x86_64__)
    if (first == 314) {
        char byte;
        errno = 0;
        if (syscall(__NR_getrandom, &byte, 1, 0) != -1 || errno != ENOSYS) {
            fprintf(stderr, "oldkernel: getrandom was not refused with ENOSYS\n");
            return 1;
        }
        errno = 0;
        if (syscall(__NR_timerfd_create, CLOCK_BOOTTIME, 0) != -1 || errno != EINVAL) {
            fprintf(stderr, "oldkernel: BOOTTIME timerfd was not refused with EINVAL\n");
            return 1;
        }
    }
#endif
    execvp(argv[2], argv + 2);
    perror("exec");
    return 127;
}
