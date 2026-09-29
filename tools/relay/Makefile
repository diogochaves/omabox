CFLAGS ?= -O2 -Wall -Wextra
omabox-relay: relay.c
	$(CC) $(CFLAGS) $(CPPFLAGS) -o $@ $< $(LDFLAGS)
clean:
	rm -f omabox-relay
.PHONY: clean
