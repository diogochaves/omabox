CFLAGS ?= -O2 -Wall -Wextra
PROTO = wlr-screencopy-unstable-v1
omabox-still: still.c $(PROTO)-client-protocol.h $(PROTO)-protocol.c
	$(CC) $(CFLAGS) $(CPPFLAGS) $(LDFLAGS) -o $@ still.c $(PROTO)-protocol.c $$(pkg-config --cflags --libs wayland-client)
$(PROTO)-client-protocol.h: $(PROTO).xml
	wayland-scanner client-header $< $@
$(PROTO)-protocol.c: $(PROTO).xml
	wayland-scanner private-code $< $@
clean:
	rm -f omabox-still $(PROTO)-client-protocol.h $(PROTO)-protocol.c
.PHONY: clean
