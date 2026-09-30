CFLAGS ?= -O2 -Wall -Wextra
omabox-events: events.c
	$(CC) $(CFLAGS) $(CPPFLAGS) -o $@ $< $(LDFLAGS)
clean:
	rm -f omabox-events
.PHONY: clean
