/* Private Linux output-failure fixture. Never linked or packaged with the app. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static size_t written;
static int owned(int fd) {
    const char *prefix = getenv("SCREENSHOTS_FIXTURE_IMAGE_PREFIX");
    char descriptor[64], path[4096];
    if (!prefix) return 0;
    snprintf(descriptor, sizeof descriptor, "/proc/self/fd/%d", fd);
    ssize_t count=readlink(descriptor,path,sizeof path-1);
    if (count<0) return 0;
    path[count]=0;
    return strncmp(path,prefix,strlen(prefix))==0;
}
static void event(void) {
    const char *path=getenv("SCREENSHOTS_FIXTURE_IMAGE_LOG");
    if (!path) return;
    int fd=open(path,O_WRONLY|O_CREAT|O_APPEND|O_CLOEXEC,0600);
    if (fd>=0) {ssize_t (*actual)(int,const void*,size_t)=dlsym(RTLD_NEXT,"write");(void)actual(fd,"injected\n",9);close(fd);}
}
ssize_t write(int fd,const void *data,size_t length) {
    ssize_t (*actual)(int,const void*,size_t)=dlsym(RTLD_NEXT,"write");
    const char *mode=getenv("SCREENSHOTS_FIXTURE_IMAGE_MODE");
    if (mode && strcmp(mode,"enospc")==0 && owned(fd)) {
        if (written>=4096) {event();errno=ENOSPC;return -1;}
        if (length>4096-written) length=4096-written;
        ssize_t result=actual(fd,data,length);
        if (result>0) written+=(size_t)result;
        return result;
    }
    return actual(fd,data,length);
}
int fsync(int fd) {
    int (*actual)(int)=dlsym(RTLD_NEXT,"fsync");
    const char *mode=getenv("SCREENSHOTS_FIXTURE_IMAGE_MODE");
    if (mode && strcmp(mode,"fsync")==0 && owned(fd)) {event();errno=EIO;return -1;}
    return actual(fd);
}
