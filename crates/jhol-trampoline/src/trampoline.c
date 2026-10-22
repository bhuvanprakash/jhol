/* Instant-Lock front door for `jhol`.
 *
 * Exec sibling jhol-trampoline for shared proof validation and bundle projection,
 * or jhol when the trampoline is unavailable. Timestamps never prove completion.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#if defined(__APPLE__)
#include <mach-o/dyld.h>
#endif

static int resolve_bin(char *out, size_t out_sz) {
#if defined(__APPLE__)
    uint32_t sz = (uint32_t)out_sz;
    if (_NSGetExecutablePath(out, &sz) != 0)
        return -1;
#else
    ssize_t n = readlink("/proc/self/exe", out, out_sz - 1);
    if (n < 0)
        return -1;
    out[n] = '\0';
#endif
    char *slash = strrchr(out, '/');
    if (!slash)
        return -1;

    strcpy(slash + 1, "jhol-trampoline");
    if (access(out, X_OK) == 0) {
        return 0;
    }

#if defined(_WIN32)
    strcpy(slash + 1, "jhol-bin.exe");
#else
    /* global-install layout: this file is `jhol` (front), the real CLI is
     * `jhol-bin`. Falling back to "jhol" here would exec THIS file again. */
    strcpy(slash + 1, "jhol-bin");
#endif
    return 0;
}

int main(int argc, char **argv) {
    (void)argc;
    char path[4096];
    if (resolve_bin(path, sizeof path) != 0) {
        fprintf(stderr, "jhol: cannot locate adjacent jhol executable\n");
        return 127;
    }
    execv(path, argv);
    perror("jhol: exec jhol");
    return 127;
}
