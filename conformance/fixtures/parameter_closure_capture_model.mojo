# A generic nested `def` captures an enclosing `var` local through its
# `{mut total}` capture list and is called with an explicit parameter.
def main():
    var total = 0

    def bump[n: Int]() {mut total}:
        total += n

    bump[5]()
    print(total)
