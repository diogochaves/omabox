CFLAGS ?= -O2 -Wall -Wextra
omabox-notices: notices.c ../common/box.h
	$(CC) $(CFLAGS) $(CPPFLAGS) $(LDFLAGS) -o $@ notices.c $$(pkg-config --cflags --libs libsystemd)
clean:
	rm -f omabox-notices
.PHONY: clean
