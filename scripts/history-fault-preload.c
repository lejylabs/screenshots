/* Linux-only fixture interposer. Never linked into or packaged with the app. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static int removed_directory = -1;
static struct stat removed_parent;

static void event(const char *message) {
    const char *path = getenv("SCREENSHOTS_FIXTURE_FAULT_LOG");
    if (!path) return;
    int fd = open(path, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0600);
    if (fd >= 0) {
        (void)write(fd, message, strlen(message));
        close(fd);
    }
}

int unlinkat(int directory, const char *name, int flags) {
    static int (*actual)(int, const char *, int);
    if (!actual) actual = dlsym(RTLD_NEXT, "unlinkat");
    int result = actual(directory, name, flags);
    const char *mode = getenv("SCREENSHOTS_FIXTURE_FAULT_MODE");
    const char *prefix = getenv("SCREENSHOTS_FIXTURE_UNLINK_PREFIX");
    if (result == 0 && flags == 0 && mode && prefix &&
        strcmp(mode, "after_unlink") == 0 && strncmp(name, prefix, strlen(prefix)) == 0) {
        if (fstat(directory, &removed_parent) == 0) removed_directory = directory;
        event("native unlink completed\n");
    }
    return result;
}

int renameat2(int source_directory, const char *source, int destination_directory,
              const char *destination, unsigned int flags) {
    static int (*actual)(int, const char *, int, const char *, unsigned int);
    if (!actual) actual = dlsym(RTLD_NEXT, "renameat2");
    const char *mode = getenv("SCREENSHOTS_FIXTURE_FAULT_MODE");
    if (mode && strcmp(mode, "stage_publish") == 0 && strncmp(source, ".screenshots-transfer-", 22) == 0) {
        event("verified staging publish rejected\n");
        errno = EIO;
        return -1;
    }
    return actual(source_directory, source, destination_directory, destination, flags);
}

int fsync(int fd) {
    static int (*actual)(int);
    if (!actual) actual = dlsym(RTLD_NEXT, "fsync");
    const char *mode = getenv("SCREENSHOTS_FIXTURE_FAULT_MODE");
    struct stat current;
    int reject = mode && strcmp(mode, "after_unlink") == 0 && fd == removed_directory &&
                 fstat(fd, &current) == 0 && S_ISDIR(current.st_mode) &&
                 current.st_dev == removed_parent.st_dev && current.st_ino == removed_parent.st_ino;
    if (mode && strcmp(mode, "parent_barrier") == 0) {
        const char *parent = getenv("SCREENSHOTS_FIXTURE_PARENT");
        char descriptor[64], path[4096];
        struct stat info;
        snprintf(descriptor, sizeof descriptor, "/proc/self/fd/%d", fd);
        ssize_t size = readlink(descriptor, path, sizeof path - 1);
        if (size >= 0) {
            path[size] = 0;
            reject = parent && strcmp(parent, path) == 0 && fstat(fd, &info) == 0 && S_ISDIR(info.st_mode);
        }
    }
    if (reject) {
        event("native parent fsync rejected\n");
        errno = EIO;
        return -1;
    }
    return actual(fd);
}
