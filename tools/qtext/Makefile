CXXFLAGS ?= -O2 -Wall -Wextra
# A library preloaded into the box's shell (Quickshell), not a program: named as the other tools are,
# so `up` and the suite find it at tools/qtext/omabox-qtext.
omabox-qtext: qtext.cpp ../common/box.h
	$(CXX) $(CXXFLAGS) $(CPPFLAGS) $(LDFLAGS) -std=c++17 -fPIC -shared -o $@ qtext.cpp $$(pkg-config --cflags --libs Qt6Quick)
clean:
	rm -f omabox-qtext
.PHONY: clean
