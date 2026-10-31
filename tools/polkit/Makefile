CFLAGS ?= -O2 -Wall -Wextra
all: omabox-polkit omabox-polkit-session.so
omabox-polkit: polkit.c ../common/box.h
	$(CC) $(CFLAGS) $(CPPFLAGS) $(LDFLAGS) -o $@ polkit.c $$(pkg-config --cflags --libs libsystemd)
omabox-polkit-session.so: session.c
	$(CC) $(CFLAGS) $(CPPFLAGS) $(LDFLAGS) -fPIC -shared -o $@ session.c
clean:
	rm -f omabox-polkit omabox-polkit-session.so
.PHONY: all clean
