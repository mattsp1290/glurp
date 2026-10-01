// Linux-only synthetic regression helper. No production fault controls exist.
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdarg.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static int step;
static int path_for(int fd, char *path, size_t size) {
    char link[64];
    snprintf(link, sizeof(link), "/proc/self/fd/%d", fd);
    ssize_t n = readlink(link, path, size - 1);
    if (n < 0) return 0;
    path[n] = 0;
    return 1;
}
static void record(const char *event) {
    const char *log = getenv("GLURP_FAULT_LOG");
    if (!log) return;
    int fd = open(log, O_WRONLY | O_CREAT | O_APPEND, 0600);
    if (fd < 0) return;
    ssize_t (*real_write)(int, const void *, size_t) = dlsym(RTLD_NEXT, "write");
    real_write(fd, event, strlen(event));
    close(fd);
}
int fsync(int fd) {
    int (*real_fsync)(int) = dlsym(RTLD_NEXT, "fsync");
    const char *mode = getenv("GLURP_FAULT_MODE");
    char path[8192];
    if (mode && (strcmp(mode, "commit-two-syncs") == 0 || strcmp(mode, "commit-reverse") == 0 || (strcmp(mode, "commit-cleanup") == 0 || strcmp(mode, "commit-partial-cleanup") == 0) || (strcmp(mode, "commit-all-transitions") == 0 || strcmp(mode, "commit-no-marker") == 0)) && path_for(fd, path, sizeof(path))) {
        size_t n = strlen(path);
        const char *suffix = "/.transaction";
        if (step == 0 && n >= strlen(suffix) && strcmp(path + n - strlen(suffix), suffix) == 0) {
            char ready[8250], committed[8250];
            snprintf(ready, sizeof(ready), "%s/ready.json", path);
            snprintf(committed, sizeof(committed), "%s/committed", path);
            if (access(ready, F_OK) != 0 && access(committed, F_OK) == 0) {
                step = 1;
                if (strcmp(mode, "commit-cleanup") == 0 || strcmp(mode, "commit-partial-cleanup") == 0) { record("commit-sync-succeeded\n"); return real_fsync(fd); }
                record("commit-sync-failed\n"); errno = EIO; return -1;
            }
        }
        if ((strcmp(mode, "commit-all-transitions") == 0 || strcmp(mode, "commit-no-marker") == 0) && step >= 1 && strstr(path, "/.transaction/0")) {
            record("restore-sync-failed\n"); errno = EIO; return -1;
        }
        if (strcmp(mode, "commit-two-syncs") == 0 && step == 1 && strstr(path, "/.transaction/0")) {
            step = 2; record("restore-sync-failed\n"); errno = EIO; return -1;
        }
    }
    if (mode && strcmp(mode, "commit-cleanup") == 0 && step >= 1 && step <= 2 && path_for(fd, path, sizeof(path)) && strstr(path, "/.transaction")) {
        step++; record("cleanup-sync-failed\n"); errno = EIO; return -1;
    }
    return real_fsync(fd);
}
int renameat(int oldfd, const char *oldname, int newfd, const char *newname) {
    int (*real_renameat)(int, const char *, int, const char *) = dlsym(RTLD_NEXT, "renameat");
    const char *mode = getenv("GLURP_FAULT_MODE");
    if (mode && (strcmp(mode, "commit-all-transitions") == 0 || strcmp(mode, "commit-no-marker") == 0) && step >= 1) {
        if (strcmp(oldname, "committed") == 0 && strcmp(newname, "ready.json") == 0) {
            record("reverse-rename-failed\n"); errno = EACCES; return -1;
        }
        if (strcmp(oldname, ".transaction") == 0 && strcmp(newname, ".recovery") == 0) {
            record("parent-rename-failed\n"); errno = EACCES; return -1;
        }
    }
    if (mode && strcmp(mode, "commit-reverse") == 0 && step == 1 && strcmp(oldname, "committed") == 0 && strcmp(newname, "ready.json") == 0) {
        step = 2; record("reverse-rename-failed\n"); errno = EACCES; return -1;
    }
    return real_renameat(oldfd, oldname, newfd, newname);
}
ssize_t write(int fd, const void *bytes, size_t count) {
    ssize_t (*real_write)(int, const void *, size_t) = dlsym(RTLD_NEXT, "write");
    const char *mode = getenv("GLURP_FAULT_MODE");
    char path[8192];
    if (mode && strcmp(mode, "slow-backup") == 0 && path_for(fd, path, sizeof(path)) && strstr(path, "/.transaction/backups/")) {
        // 64 MiB completes in >5s without cancellation checks. Checked copying
        // reaches timeout/cancellation after only a few bounded chunks.
        usleep(5000);
    }
    return real_write(fd, bytes, count);
}
ssize_t read(int fd, void *bytes, size_t count) {
    ssize_t (*real_read)(int, void *, size_t) = dlsym(RTLD_NEXT, "read");
    const char *mode = getenv("GLURP_FAULT_MODE");
    char path[8192];
    if (mode && strcmp(mode, "slow-compare") == 0 && path_for(fd, path, sizeof(path)) && strstr(path, "/data/glurp/hosts/lab/codex/session_index.jsonl")) {
        if (step == 0) { step = 1; record("comparison-started\n"); }
        usleep(2000);
    }
    return real_read(fd, bytes, count);
}

static int marker_fault(const char *name, int flags) {
    const char *mode = getenv("GLURP_FAULT_MODE");
    if (mode && strcmp(mode, "commit-no-marker") == 0 && step >= 1 &&
        (flags & O_CREAT) && strcmp(name, ".rollback-required") == 0) {
        record("marker-create-failed\n"); errno = EIO; return 1;
    }
    return 0;
}
int openat(int fd, const char *name, int flags, ...) {
    int (*real_openat)(int, const char *, int, ...) = dlsym(RTLD_NEXT, "openat");
    mode_t permissions = 0;
    if (flags & O_CREAT) { va_list args; va_start(args, flags); permissions = va_arg(args, int); va_end(args); }
    if (marker_fault(name, flags)) return -1;
    return real_openat(fd, name, flags, permissions);
}
int openat64(int fd, const char *name, int flags, ...) {
    int (*real_openat)(int, const char *, int, ...) = dlsym(RTLD_NEXT, "openat64");
    mode_t permissions = 0;
    if (flags & O_CREAT) { va_list args; va_start(args, flags); permissions = va_arg(args, int); va_end(args); }
    if (marker_fault(name, flags)) return -1;
    return real_openat(fd, name, flags, permissions);
}
int unlinkat(int fd, const char *name, int flags) {
    int (*real_unlinkat)(int, const char *, int) = dlsym(RTLD_NEXT, "unlinkat");
    const char *mode = getenv("GLURP_FAULT_MODE");
    char path[8192];
    if (mode && strcmp(mode, "commit-partial-cleanup") == 0 && step >= 1 &&
        strcmp(name, "1") == 0 && path_for(fd, path, sizeof(path)) && strstr(path, "/.transaction/backups")) {
        record("partial-cleanup-failed\n"); errno = EIO; return -1;
    }
    return real_unlinkat(fd, name, flags);
}
