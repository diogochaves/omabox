CFLAGS ?= -O2 -Wall -Wextra
PROTO = wlr-virtual-pointer-unstable-v1
omabox-pointer: pointer.c $(PROTO)-client-protocol.h $(PROTO)-protocol.c
	$(CC) $(CFLAGS) $(CPPFLAGS) -o $@ pointer.c $(PROTO)-protocol.c $$(pkg-config --cflags --libs wayland-client) $(LDFLAGS)
$(PROTO)-client-protocol.h: $(PROTO).xml
	wayland-scanner client-header $< $@
$(PROTO)-protocol.c: $(PROTO).xml
	wayland-scanner private-code $< $@
clean:
	rm -f omabox-pointer $(PROTO)-client-protocol.h $(PROTO)-protocol.c
.PHONY: clean
