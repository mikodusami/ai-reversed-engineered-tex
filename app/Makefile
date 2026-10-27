# Builds build/RTeX.app: Objective-C++ front end + Rust core (static library).
#   make            # release build
#   make run        # build and launch
#   make clean

CARGO    ?= $(HOME)/.cargo/bin/cargo
ROOT     := $(abspath ..)
RUST_LIB := $(ROOT)/target/release/librtex_jobs.a
BUILD    := build
APP      := $(BUILD)/RTeX.app
BIN      := $(APP)/Contents/MacOS/RTeX
SRCS     := $(wildcard src/*.mm)
OBJS     := $(patsubst src/%.mm,$(BUILD)/obj/%.o,$(SRCS))

CXX      := clang++
CXXFLAGS := -std=c++20 -fobjc-arc -O2 -Wall -Wextra -Wno-unused-parameter \
            -Iinclude -mmacosx-version-min=13.0 -arch arm64
LDFLAGS  := -mmacosx-version-min=13.0 -arch arm64 \
            -framework Cocoa -framework PDFKit -framework UniformTypeIdentifiers \
            -lc -lm

.PHONY: all run clean rust test FORCE

all: $(APP)

rust: FORCE
	cd $(ROOT) && $(CARGO) build --release -p rtex-jobs

$(RUST_LIB): rust

$(BUILD)/obj/%.o: src/%.mm $(wildcard src/*.h) include/rtex.h
	@mkdir -p $(dir $@)
	$(CXX) $(CXXFLAGS) -c $< -o $@

$(BIN): $(OBJS) $(RUST_LIB)
	@mkdir -p $(dir $@)
	$(CXX) $(OBJS) $(RUST_LIB) $(LDFLAGS) -o $@

$(APP): $(BIN) Info.plist
	@mkdir -p $(APP)/Contents/Resources
	cp Info.plist $(APP)/Contents/Info.plist
	printf 'APPL????' > $(APP)/Contents/PkgInfo
	codesign --force --sign - --timestamp=none $(APP)
	@echo "built $(APP)"

TEST_OBJS := $(filter-out $(BUILD)/obj/main.o,$(OBJS))

$(BUILD)/editor_test: tests/editor_test.mm $(TEST_OBJS) $(RUST_LIB)
	$(CXX) $(CXXFLAGS) -Isrc tests/editor_test.mm $(TEST_OBJS) $(RUST_LIB) $(LDFLAGS) -o $@

$(BUILD)/document_test: tests/document_test.mm $(TEST_OBJS) $(RUST_LIB)
	$(CXX) $(CXXFLAGS) -Isrc tests/document_test.mm $(TEST_OBJS) $(RUST_LIB) $(LDFLAGS) -o $@

$(BUILD)/jobs_test: tests/jobs_test.mm $(TEST_OBJS) $(RUST_LIB)
	$(CXX) $(CXXFLAGS) -Isrc tests/jobs_test.mm $(TEST_OBJS) $(RUST_LIB) $(LDFLAGS) -o $@

test: $(BUILD)/editor_test $(BUILD)/document_test $(BUILD)/jobs_test
	$(BUILD)/editor_test
	$(BUILD)/document_test
	$(BUILD)/jobs_test

run: $(APP)
	open $(APP)

clean:
	rm -rf $(BUILD)

FORCE:
