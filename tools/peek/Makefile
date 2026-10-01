CFLAGS ?= -O2 -Wall -Wextra
PROTOS = wlr-screencopy-unstable-v1 xdg-shell
GEN = $(PROTOS:%=%-client-protocol.h) $(PROTOS:%=%-protocol.c)
omabox-peek: peek.c font8x8.h $(GEN)
	$(CC) $(CFLAGS) $(CPPFLAGS) $(LDFLAGS) -o $@ peek.c $(PROTOS:%=%-protocol.c) $$(pkg-config --cflags --libs wayland-client)
%-client-protocol.h: %.xml
	wayland-scanner client-header $< $@
%-protocol.c: %.xml
	wayland-scanner private-code $< $@
clean:
	rm -f omabox-peek $(GEN)
.PHONY: clean
