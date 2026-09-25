CFLAGS ?= -O2 -Wall -Wextra
PROTO = virtual-keyboard-unstable-v1
omabox-keyboard: keyboard.c $(PROTO)-client-protocol.h $(PROTO)-protocol.c
	$(CC) $(CFLAGS) $(CPPFLAGS) -o $@ keyboard.c $(PROTO)-protocol.c $$(pkg-config --cflags --libs wayland-client xkbcommon) $(LDFLAGS)
$(PROTO)-client-protocol.h: $(PROTO).xml
	wayland-scanner client-header $< $@
$(PROTO)-protocol.c: $(PROTO).xml
	wayland-scanner private-code $< $@
clean:
	rm -f omabox-keyboard $(PROTO)-client-protocol.h $(PROTO)-protocol.c
.PHONY: clean
