CFLAGS ?= -O2 -Wall -Wextra
omabox-events: events.c ../common/box.h
	$(CC) $(CFLAGS) $(CPPFLAGS) $(LDFLAGS) -o $@ events.c
clean:
	rm -f omabox-events
.PHONY: clean
