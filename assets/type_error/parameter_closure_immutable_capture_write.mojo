# expect: must be mutable for in-place operator destination
# A generic closure's `imm` capture of an enclosing argument cannot be
# written in place; upstream reports the same text.
def run(base: Int):
    def bump[n: Int]() {imm base}:
        base += n
    bump[5]()
    print(base)
def main():
    run(10)
