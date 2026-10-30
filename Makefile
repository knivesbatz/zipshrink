# ZipShrink: one set of 32-bit Windows executables for Windows 95 through 11.
#
#   make                 zipshrink.exe and zipshrink-gui.exe in build/
#   make test            the test suite (needs Windows, or Wine on Linux)
#   make check-imports   confirms every imported function exists on Windows 95
#
# Built with MinGW-w64 (i686-w64-mingw32-gcc) and no C runtime library: the
# runtimes of current compilers import functions Windows 95 does not have.

CROSS   ?= i686-w64-mingw32-
CC      := $(CROSS)gcc
WINDRES := $(CROSS)windres
OBJDUMP := $(CROSS)objdump
B       ?= build

VERSION := $(shell sed -n 's/^\#define ZS_VERSION "\(.*\)"/\1/p' src/version.h)

CFLAGS  ?= -Os
CFLAGS  += -std=gnu99 -Wall -Wextra -Wno-unused-parameter -Wno-cast-function-type -march=i386 -mtune=pentium \
           -ffreestanding -fno-builtin -fno-tree-loop-distribute-patterns -fno-stack-protector \
           -fno-asynchronous-unwind-tables -fno-ident -DNDEBUG \
           -DMINIZ_NO_STDIO -DMINIZ_NO_TIME -DMINIZ_NO_ARCHIVE_APIS -DMINIZ_NO_ZLIB_APIS \
           -Isrc -Ivendor/miniz
# Windows 4.0 (95 / NT 4) in the PE header; later versions refuse nothing older.
LDFLAGS := -nostdlib -nostartfiles -s -Wl,--major-os-version,4 -Wl,--minor-os-version,0 \
           -Wl,--major-subsystem-version,4 -Wl,--minor-subsystem-version,0 \
           -Wl,--dynamicbase,--nxcompat,--no-insert-timestamp
LIBS    := -lkernel32 -lgcc
GUILIBS := -luser32 -lgdi32 -lcomctl32 -lcomdlg32 -ladvapi32 -lkernel32 -lgcc

ENGINE  := base hash mzglue plat failpoint zipfmt journal scan space split verify engine update
MINIZ   := miniz_tdef miniz_tinfl

REL_OBJ  := $(addprefix $(B)/rel/,$(addsuffix .o,$(ENGINE) $(MINIZ)))
TEST_OBJ := $(addprefix $(B)/fp/,$(addsuffix .o,$(ENGINE) $(MINIZ)))

all: $(B)/zipshrink.exe $(B)/zipshrink-gui.exe

$(B)/rel/%.o: src/%.c src/*.h
	@mkdir -p $(@D)
	$(CC) $(CFLAGS) -c $< -o $@
$(B)/rel/%.o: vendor/miniz/%.c vendor/miniz/*.h
	@mkdir -p $(@D)
	$(CC) $(CFLAGS) -c $< -o $@
$(B)/fp/%.o: src/%.c src/*.h
	@mkdir -p $(@D)
	$(CC) $(CFLAGS) -DZS_FAILPOINTS -c $< -o $@
$(B)/fp/%.o: vendor/miniz/%.c vendor/miniz/*.h
	@mkdir -p $(@D)
	$(CC) $(CFLAGS) -DZS_FAILPOINTS -c $< -o $@

$(B)/res/%.o: res/%.rc res/* src/version.h assets/zipshrink.ico
	@mkdir -p $(@D)
	$(WINDRES) -I res -I src -i $< -o $@

$(B)/zipshrink.exe: $(B)/rel/cli.o $(REL_OBJ) $(B)/res/cli.o
	$(CC) $(LDFLAGS) -Wl,--subsystem,console:4.0 -Wl,-e,_zs_cli_start $^ $(LIBS) -o $@

$(B)/zipshrink-gui.exe: $(B)/rel/gui.o $(B)/rel/y2k.o $(B)/rel/uw.o $(REL_OBJ) $(B)/res/gui.o
	$(CC) $(LDFLAGS) -Wl,--subsystem,windows:4.0 -Wl,-e,_zs_gui_start $^ $(GUILIBS) -o $@

# The command line program with fault injection, for the crash tests.
$(B)/zipshrink-fp.exe: $(B)/fp/cli.o $(TEST_OBJ)
	$(CC) $(LDFLAGS) -Wl,--subsystem,console:4.0 -Wl,-e,_zs_cli_start $^ $(LIBS) -o $@

# In-process unit and fault tests.
$(B)/zs-tests.exe: $(B)/fp/tests.o $(TEST_OBJ)
	$(CC) $(LDFLAGS) -Wl,--subsystem,console:4.0 -Wl,-e,_zs_test_start $^ $(LIBS) -o $@
$(B)/fp/tests.o: tests/tests.c src/*.h
	@mkdir -p $(@D)
	$(CC) $(CFLAGS) -DZS_FAILPOINTS -c $< -o $@

test: $(B)/zs-tests.exe $(B)/zipshrink-fp.exe $(B)/zipshrink.exe
	python3 tests/run.py --build $(B)

check-imports: all
	python3 scripts/check_imports.py $(B)/zipshrink.exe $(B)/zipshrink-gui.exe

clean:
	rm -rf $(B)

.PHONY: all test check-imports clean
