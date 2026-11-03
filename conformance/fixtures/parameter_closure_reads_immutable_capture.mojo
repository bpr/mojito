# A generic nested `def` reads an immutable enclosing argument alongside its
# mutable capture of a `var` local (pinned Mojo: prints 15).
def run(base: Int):
    var total = 0
    def bump[n: Int]() {mut total, imm base}:
        total += n + base
    bump[5]()
    print(total)
def main():
    run(10)
