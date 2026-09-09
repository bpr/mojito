# expect: must be mutable for in-place operator destination
# A `@__parameter` closure captures an immutable enclosing parameter as
# `imm`; writing it in place rejects with upstream's text.
def run(base: Int):
    @__parameter
    def bump[n: Int]():
        base += n
    bump[5]()
    print(base)
def main():
    run(10)
