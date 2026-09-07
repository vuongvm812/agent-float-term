/* Native foreground job with opt-in inert workers: no AI tools or network. */
#define _DEFAULT_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <termios.h>
#include <unistd.h>

int main(int argc, char **argv) {
    struct termios saved, raw;
    const char key[] = "\033[18~";
    const char *name = strrchr(argv[0], '/');
    size_t matched = 0;
    unsigned count = 0;
    unsigned char byte;
    int result = 0;
    int workers = 0, lifetime[2] = {-1, -1}, ready[2] = {-1, -1};
    pid_t children[3];
    const char *requested = getenv("AFT_TEST_WORKERS");

    if (argc != 1 || tcgetattr(STDIN_FILENO, &saved) != 0)
        return 1;
    if (requested) {
        if (strcmp(requested, "2") != 0 && strcmp(requested, "3") != 0)
            return 1;
        if (pipe(lifetime) != 0 || pipe(ready) != 0)
            return 1;
        long max_fd = sysconf(_SC_OPEN_MAX);
        if (max_fd < 0)
            return 1;
        for (int i = 0; i < atoi(requested); ++i) {
            pid_t child = fork();
            if (child < 0)
                return 1; /* Process exit closes the sole lifetime writer. */
            if (child == 0) {
                int sink = open("/dev/null", O_WRONLY);
                if (sink < 0 || dup2(lifetime[0], STDIN_FILENO) < 0 ||
                    dup2(sink, STDOUT_FILENO) < 0)
                    _exit(1);
                /* No hidden TTY copies, and no worker may keep its siblings alive. */
                for (int fd = 3; fd < max_fd; ++fd)
                    if (fd != ready[1])
                        close(fd);
                if (isatty(STDIN_FILENO) || isatty(STDOUT_FILENO) ||
                    !isatty(STDERR_FILENO) || write(ready[1], "r", 1) != 1)
                    _exit(1);
                close(ready[1]);
                ssize_t size;
                do {
                    size = read(STDIN_FILENO, &byte, 1);
                } while (size > 0 || (size < 0 && errno == EINTR));
                _exit(size < 0);
            }
            children[workers++] = child;
        }
        close(lifetime[0]);
        close(ready[1]);
        for (int i = 0; i < workers; ++i) {
            ssize_t size;
            do {
                size = read(ready[0], &byte, 1);
            } while (size < 0 && errno == EINTR);
            if (size != 1)
                return 1;
        }
        close(ready[0]);
    }
    name = name ? name + 1 : argv[0];
    raw = saved;
    cfmakeraw(&raw);
    if (tcsetattr(STDIN_FILENO, TCSANOW, &raw) != 0)
        return 1;
    setvbuf(stdout, NULL, _IONBF, 0);
    for (int i = 0; i < workers; ++i)
        printf("AFT_WORKER pid=%ld pgid=%ld\r\n", (long)children[i], (long)getpgrp());
    printf("AFT_READY %s pid=%ld\r\n", name, (long)getpid());
    for (;;) {
        ssize_t size = read(STDIN_FILENO, &byte, 1);
        if (size < 0 && errno == EINTR)
            continue;
        if (size <= 0) {
            result = size < 0;
            break;
        }
        if (byte == 'q')
            break;
        if (byte == (unsigned char)key[matched]) {
            if (++matched == sizeof(key) - 1) {
                printf("AFT_KEY %s count=%u pid=%ld\r\n", name, ++count,
                       (long)getpid());
                matched = 0;
            }
        } else {
            matched = byte == (unsigned char)key[0] ? 1 : 0;
        }
    }
    if (workers) {
        close(lifetime[1]);
        for (int i = 0; i < workers; ++i) {
            int status;
            pid_t waited;
            do {
                waited = waitpid(children[i], &status, 0);
            } while (waited < 0 && errno == EINTR);
            if (waited < 0 || !WIFEXITED(status) || WEXITSTATUS(status) != 0)
                result = 1;
        }
        if (!result)
            printf("AFT_WORKERS_REAPED count=%d\r\n", workers);
    }
    if (tcsetattr(STDIN_FILENO, TCSANOW, &saved) != 0)
        result = 1;
    return result;
}
