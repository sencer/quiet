#define _GNU_SOURCE
#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/socket.h>
#include <sys/un.h>

#define QUIET_DEST "org.freedesktop.Notifications"
#define QUIET_PATH "/org/freedesktop/Notifications"
#define QUIET_IFACE "org.freedesktop.Notifications"

static void print_help(const char *progname) {
    printf("Usage: %s [OPTIONS]\n\n"
           "Zero-overhead standalone C client for Quiet notification daemon.\n"
           "Direct wire-protocol implementation over UNIX domain socket (zero external libs).\n\n"
           "Options:\n"
           "  -t, --toggle    Toggle notification center overlay (default, async)\n"
           "  -w, --wait      Toggle notification center and wait for ACK (sync)\n"
           "  -n, --count     Print current notification count to stdout\n"
           "  -u, --urgency   Print current notification urgency (0=low, 1=normal, 2=critical)\n"
           "  -k, --kill      Terminate running Quiet daemon via D-Bus Quit\n"
           "  -h, --help      Show this help message\n",
           progname);
}

static inline int append_field(uint8_t *buf, int offset, uint8_t code, char type, const char *val) {
    while (offset % 8 != 0) buf[offset++] = 0;
    buf[offset++] = code;
    buf[offset++] = 1;
    buf[offset++] = type;
    buf[offset++] = 0;
    uint32_t len = (uint32_t)strlen(val);
    *(uint32_t*)(buf + offset) = len;
    offset += 4;
    memcpy(buf + offset, val, len + 1);
    offset += len + 1;
    return offset;
}

static inline int build_method_call(
    uint8_t *buf,
    uint32_t serial,
    uint8_t flags,
    const char *dest,
    const char *path,
    const char *iface,
    const char *member
) {
    buf[0] = 'l'; // Little-endian
    buf[1] = 1;   // METHOD_CALL
    buf[2] = flags;
    buf[3] = 1;   // Major protocol version 1
    *(uint32_t*)(buf + 4) = 0; // Body length
    *(uint32_t*)(buf + 8) = serial;
    int offset = 16;
    offset = append_field(buf, offset, 1, 'o', path);
    offset = append_field(buf, offset, 3, 's', member);
    if (iface) offset = append_field(buf, offset, 2, 's', iface);
    if (dest)  offset = append_field(buf, offset, 6, 's', dest);
    *(uint32_t*)(buf + 12) = offset - 16;
    while (offset % 8 != 0) buf[offset++] = 0;
    return offset;
}

int main(int argc, char *argv[]) {
    if (argc > 1 && (strcmp(argv[1], "-h") == 0 || strcmp(argv[1], "--help") == 0)) {
        print_help(argv[0]);
        _exit(0);
    }

    const char *member = "ShowNotifications";
    int expect_reply = 1;
    int mode = 0; // 0 = toggle/quit, 1 = count, 2 = urgency

    if (argc > 1) {
        if (strcmp(argv[1], "-t") == 0 || strcmp(argv[1], "--toggle") == 0) {
            member = "ShowNotifications";
            expect_reply = 1;
        } else if (strcmp(argv[1], "-w") == 0 || strcmp(argv[1], "--wait") == 0) {
            member = "ShowNotifications";
            expect_reply = 1;
        } else if (strcmp(argv[1], "-k") == 0 || strcmp(argv[1], "--kill") == 0) {
            member = "Quit";
            expect_reply = 0;
        } else if (strcmp(argv[1], "-n") == 0 || strcmp(argv[1], "--count") == 0) {
            member = "ShowNotificationCount";
            expect_reply = 1;
            mode = 1;
        } else if (strcmp(argv[1], "-u") == 0 || strcmp(argv[1], "--urgency") == 0) {
            member = "ShowNotificationCount";
            expect_reply = 1;
            mode = 2;
        } else {
            print_help(argv[0]);
            _exit(1);
        }
    }

    // Connect to session D-Bus UNIX domain socket
    int fd = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (fd < 0) {
        perror("socket");
        _exit(1);
    }

    struct sockaddr_un addr;
    memset(&addr, 0, sizeof(addr));
    addr.sun_family = AF_UNIX;
    const char *runtime_dir = getenv("XDG_RUNTIME_DIR");
    if (runtime_dir) {
        snprintf(addr.sun_path, sizeof(addr.sun_path), "%s/bus", runtime_dir);
    } else {
        const char *dbus_addr = getenv("DBUS_SESSION_BUS_ADDRESS");
        if (dbus_addr && strncmp(dbus_addr, "unix:path=", 10) == 0) {
            snprintf(addr.sun_path, sizeof(addr.sun_path), "%s", dbus_addr + 10);
            char *comma = strchr(addr.sun_path, ',');
            if (comma) *comma = '\0';
        } else {
            snprintf(addr.sun_path, sizeof(addr.sun_path), "/run/user/%u/bus", getuid());
        }
    }

    if (connect(fd, (struct sockaddr*)&addr, sizeof(addr)) < 0) {
        fprintf(stderr, "Failed to connect to D-Bus at %s\n", addr.sun_path);
        close(fd);
        _exit(1);
    }

    // Compose pipelined buffer:
    // [SASL Handshake (29 bytes)] + [Hello (128 bytes)] + [Target Method Call]
    uint8_t req[1024];
    static const unsigned char handshake[] =
        "\x00\x41\x55\x54\x48\x20\x45\x58\x54\x45\x52\x4e\x41\x4c\x0d\x0a\x44\x41\x54\x41\x0d\x0a\x42\x45\x47\x49\x4e\x0d\x0a";
    int req_len = sizeof(handshake) - 1;
    memcpy(req, handshake, req_len);

    // Hello message (serial = 1, flags = 0)
    int hello_len = build_method_call(
        req + req_len,
        1,
        0,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "Hello"
    );
    req_len += hello_len;

    // Target method call (serial = 2, flags = NO_REPLY_EXPECTED if !expect_reply)
    uint8_t flags = expect_reply ? 0 : 0x01;
    int call_len = build_method_call(
        req + req_len,
        2,
        flags,
        QUIET_DEST,
        QUIET_PATH,
        QUIET_IFACE,
        member
    );
    req_len += call_len;

    // Send the entire sequence in a single write call!
    if (write(fd, req, req_len) < 0) {
        perror("write");
        close(fd);
        _exit(1);
    }

    // Read responses
    uint8_t resp[2048];
    int resp_len = 0;

    while (resp_len < (int)sizeof(resp)) {
        int n = read(fd, resp + resp_len, sizeof(resp) - resp_len);
        if (n <= 0) break;
        resp_len += n;

        // Check if we got what we need:
        for (int i = 0; i <= resp_len - 16; i++) {
            if (resp[i] != 'l') continue;
            uint8_t msg_type = resp[i + 1];

            // Method error from daemon
            if (msg_type == 3) {
                uint32_t fields_len = *(uint32_t*)(resp + i + 12);
                if (fields_len >= 8 && memmem(resp + i + 16, fields_len, "\x05\x01\x75\x00\x02\x00\x00\x00", 8)) {
                    fprintf(stderr, "Quiet D-Bus call failed (service not running or error returned)\n");
                    close(fd);
                    _exit(1);
                }
            }

            // Method return
            if (msg_type == 2) {
                uint32_t fields_len = *(uint32_t*)(resp + i + 12);
                int is_hello_reply = (fields_len >= 8 && memmem(resp + i + 16, fields_len, "\x05\x01\x75\x00\x01\x00\x00\x00", 8) != NULL);
                int is_call_reply = (fields_len >= 8 && memmem(resp + i + 16, fields_len, "\x05\x01\x75\x00\x02\x00\x00\x00", 8) != NULL);

                if (!expect_reply && is_hello_reply) {
                    // Fire-and-forget: once Hello reply is received, the broker has read the entire pipelined buffer
                    close(fd);
                    _exit(0);
                }

                if (is_call_reply) {
                    if (mode == 1 || mode == 2) {
                        uint32_t body_len = *(uint32_t*)(resp + i + 4);
                        uint32_t fields_len = *(uint32_t*)(resp + i + 12);
                        int header_total = 16 + ((fields_len + 7) & ~7);
                        if (body_len >= 8 && i + header_total + 8 <= resp_len) {
                            uint32_t count = *(uint32_t*)(resp + i + header_total);
                            uint32_t urgency = *(uint32_t*)(resp + i + header_total + 4);
                            if (mode == 1) {
                                printf("%u\n", count);
                            } else {
                                printf("%u\n", urgency);
                            }
                            close(fd);
                            _exit(0);
                        }
                    } else {
                        // Sync toggle reply received
                        close(fd);
                        _exit(0);
                    }
                }
            }
        }
    }

    close(fd);
    _exit(expect_reply ? 1 : 0);
}
