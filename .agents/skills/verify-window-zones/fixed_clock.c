#define _GNU_SOURCE
#include <limits.h>
#include <stdio.h>
#include <string.h>
#include <sys/syscall.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>

/* Freeze application wall time, not monotonic deadlines or the benchmark timer. */
#ifndef CLOCK_PROBE
static int freeze;

__attribute__((constructor)) static void select_application(void) {
    char executable[PATH_MAX];
    ssize_t size = readlink("/proc/self/exe", executable, sizeof executable - 1);
    if (size < 0) return;
    executable[size] = '\0';
    const char *name = strrchr(executable, '/');
    name = name ? name + 1 : executable;
    freeze = strcmp(name, "window_zones") == 0 || strcmp(name, "clock-probe") == 0;
}

int clock_gettime(clockid_t clock, struct timespec *value) {
    if (freeze && (clock == CLOCK_REALTIME || clock == CLOCK_REALTIME_COARSE)) {
        value->tv_sec = 1704067200;
        value->tv_nsec = 0;
        return 0;
    }
    return (int)syscall(SYS_clock_gettime, clock, value);
}

time_t time(time_t *value) {
    struct timespec now = {1704067200, 0};
    if (!freeze && syscall(SYS_clock_gettime, CLOCK_REALTIME, &now)) return (time_t)-1;
    if (value) *value = now.tv_sec;
    return now.tv_sec;
}

int gettimeofday(struct timeval *value, void *zone) {
    if (!freeze) return (int)syscall(SYS_gettimeofday, value, zone);
    value->tv_sec = 1704067200;
    value->tv_usec = 0;
    if (zone) {
        ((struct timezone *)zone)->tz_minuteswest = 0;
        ((struct timezone *)zone)->tz_dsttime = 0;
    }
    return 0;
}
#else
/* Built separately: its libc calls must resolve through the preloaded library. */
int main(void) {
    struct timespec wall, first, next;
    struct timeval tv;
    if (clock_gettime(CLOCK_REALTIME, &wall) || gettimeofday(&tv, NULL) ||
        clock_gettime(CLOCK_MONOTONIC, &first)) return 1;
    next = first;
    for (int i = 0; i < 100000 && next.tv_sec == first.tv_sec && next.tv_nsec == first.tv_nsec; ++i) {
        if (clock_gettime(CLOCK_MONOTONIC, &next)) return 1;
    }
    printf("{\"realtime_seconds\":%lld,\"realtime_nanoseconds\":%ld,"
           "\"time_seconds\":%lld,\"gettimeofday_seconds\":%lld,"
           "\"gettimeofday_microseconds\":%ld,\"monotonic_advances\":%s}\n",
           (long long)wall.tv_sec, wall.tv_nsec, (long long)time(NULL),
           (long long)tv.tv_sec, (long)tv.tv_usec,
           next.tv_sec > first.tv_sec || next.tv_nsec > first.tv_nsec ? "true" : "false");
    return 0;
}
#endif
