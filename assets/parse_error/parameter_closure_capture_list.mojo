# expect: '@__parameter' closures are no longer supported; remove the decorator and declare the captures in a capture list like '{var}' or '{mut x}'
# The removed `@__parameter` closure decorator rejects, with or without a
# capture list beside it.
def main():
    var total = 0
    @__parameter
    def bump[n: Int]() {mut total}:
        total += n
    bump[5]()
    print(total)
