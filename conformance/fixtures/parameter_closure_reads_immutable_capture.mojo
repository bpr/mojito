# A `@__parameter` closure reads an immutable enclosing parameter alongside
# its implicit mutable capture of a `var` local (pinned Mojo: prints 15).
def run(base: Int):
    var total = 0
    @__parameter
    def bump[n: Int]():
        total += n + base
    bump[5]()
    print(total)
def main():
    run(10)
