CC ?= gcc
CFLAGS ?= -O3 -march=native -flto -fno-plt -fno-stack-protector -Wall -Wextra
LDFLAGS ?= -Wl,-O1,--as-needed,-z,now -s
PREFIX ?= $(HOME)/.local

.PHONY: all static clean install

all: quietctl

quietctl: quietctl.c
	$(CC) $(CFLAGS) $(LDFLAGS) quietctl.c -o quietctl

static: quietctl_static

quietctl_static: quietctl.c
	$(CC) -static $(CFLAGS) -s quietctl.c -o quietctl_static

install: quietctl
	install -d $(DESTDIR)$(PREFIX)/bin
	install -m 755 quietctl $(DESTDIR)$(PREFIX)/bin/quietctl

clean:
	rm -f quietctl quietctl_static
