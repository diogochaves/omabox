CFLAGS ?= -O2 -Wall -Wextra
omabox-relay: relay.c
	$(CC) $(CFLAGS) $(CPPFLAGS) $(LDFLAGS) -o $@ $<
clean:
	rm -f omabox-relay
.PHONY: clean
