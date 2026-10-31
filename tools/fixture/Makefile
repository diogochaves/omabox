CFLAGS ?= -O2 -Wall -Wextra
PROTOS = xdg-shell
GEN = $(PROTOS:%=%-client-protocol.h) $(PROTOS:%=%-protocol.c)
omabox-fixture: fixture.c ../common/box.h $(GEN)
	$(CC) $(CFLAGS) $(CPPFLAGS) $(LDFLAGS) -o $@ fixture.c $(PROTOS:%=%-protocol.c) $$(pkg-config --cflags --libs wayland-client xkbcommon)
%-client-protocol.h: %.xml
	wayland-scanner client-header $< $@
%-protocol.c: %.xml
	wayland-scanner private-code $< $@
clean:
	rm -f omabox-fixture $(GEN)
.PHONY: clean
