CFLAGS ?= -O2 -Wall -Wextra
PROTOS = wlr-layer-shell-unstable-v1 xdg-shell idle-inhibit-unstable-v1
GEN = $(PROTOS:%=%-client-protocol.h) $(PROTOS:%=%-protocol.c)
omabox-idlehold: idlehold.c ../common/box.h ../common/roundtrip.h $(GEN)
	$(CC) $(CFLAGS) $(CPPFLAGS) $(LDFLAGS) -o $@ idlehold.c $(PROTOS:%=%-protocol.c) $$(pkg-config --cflags --libs wayland-client)
%-client-protocol.h: %.xml
	wayland-scanner client-header $< $@
%-protocol.c: %.xml
	wayland-scanner private-code $< $@
clean:
	rm -f omabox-idlehold $(GEN)
.PHONY: clean
