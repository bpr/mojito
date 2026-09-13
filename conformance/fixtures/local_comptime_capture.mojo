# A function-local `comptime` constant read from a nested `def`: Mojito binds
# it like any immutable local and so wants it in the capture list, while the
# pin treats it as a compile-time constant and rejects naming it there.
def main():
    comptime counter: Int = 1
    def inner() {imm counter}:
        print(counter + 3)
    inner()
