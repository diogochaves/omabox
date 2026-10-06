CFLAGS ?= -O2 -Wall -Wextra
PROTOS = wlr-screencopy-unstable-v1 xdg-output-unstable-v1
GEN = $(PROTOS:%=%-client-protocol.h) $(PROTOS:%=%-protocol.c)
omabox-still: still.c ../common/box.h ../common/roundtrip.h $(GEN)
	$(CC) $(CFLAGS) $(CPPFLAGS) $(LDFLAGS) -o $@ still.c $(PROTOS:%=%-protocol.c) $$(pkg-config --cflags --libs wayland-client) -lm
%-client-protocol.h: %.xml
	wayland-scanner client-header $< $@
%-protocol.c: %.xml
	wayland-scanner private-code $< $@
clean:
	rm -f omabox-still $(GEN)
.PHONY: clean
