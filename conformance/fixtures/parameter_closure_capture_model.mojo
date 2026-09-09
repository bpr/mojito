# A `@__parameter` closure captures an enclosing `var` local implicitly
# mutable and takes no capture list (pinned Mojo a79fbdf59f2, 2026-09-08).
def main():
    var total = 0

    @__parameter
    def bump[n: Int]():
        total += n

    bump[5]()
    print(total)
