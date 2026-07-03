# Shared variables and targets included by Makefile.

PREFIX := /usr/local
BINDIR := $(PREFIX)/bin
DATADIR := $(PREFIX)/share

install:
	install -d $(BINDIR)
	install -m 755 target/release/$(NAME) $(BINDIR)/

uninstall:
	rm -f $(BINDIR)/$(NAME)
