/* Herdr Desktop spec 007 latency instrumentation for a PRIVATE build of wtype 0.4.
 * Inactive unless HD_LATENCY_LOG names a file that does not exist yet (O_EXCL): then every
 * type_keycode press stamps CLOCK_MONOTONIC immediately before the pressed virtual-keyboard
 * request and appends one line AFTER that request's roundtrip. Nothing else changes. */
#ifndef HD_LATENCY_H
#define HD_LATENCY_H
#include <fcntl.h>
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
#include <unistd.h>

static inline int64_t hd_latency_now_ns(void)
{
	struct timespec ts;
	if (clock_gettime(CLOCK_MONOTONIC, &ts) != 0)
		return -1;
	return (int64_t)ts.tv_sec * 1000000000 + (int64_t)ts.tv_nsec;
}

static inline int64_t hd_latency_res_ns(void)
{
	struct timespec ts;
	if (clock_getres(CLOCK_MONOTONIC, &ts) != 0)
		return -1;
	return (int64_t)ts.tv_sec * 1000000000 + (int64_t)ts.tv_nsec;
}

/* -1: disabled (variable unset). Exits when set but unusable: never measure silently. */
static inline int hd_latency_fd(void)
{
	static int fd = -2;
	if (fd == -2) {
		const char *path = getenv("HD_LATENCY_LOG");
		if (path == NULL || path[0] == '\0') {
			fd = -1;
		} else {
			fd = open(path, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0600);
			if (fd < 0) {
				fprintf(stderr, "hd-latency: cannot create %s\n", path);
				exit(70);
			}
		}
	}
	return fd;
}

static inline void hd_latency_line(const char *event, unsigned index, unsigned key_code,
				   int64_t ns)
{
	int fd = hd_latency_fd();
	if (fd < 0)
		return;
	char line[160];
	int n = snprintf(line, sizeof line,
			 "hd-latency v1 %s index=%u keycode=%u clock=CLOCK_MONOTONIC res_ns=%" PRId64
			 " ns=%" PRId64 "\n",
			 event, index, key_code, hd_latency_res_ns(), ns);
	if (n <= 0 || n >= (int)sizeof line || write(fd, line, (size_t)n) != n) {
		fprintf(stderr, "hd-latency: write failed\n");
		exit(70);
	}
}
#endif
