# expect: expected ':' in function definition
# A `@__parameter` def captures implicitly and takes no capture list;
# upstream reads the `{` as a missing body.
def main():
    var total = 0
    @__parameter
    def bump[n: Int]() {mut total}:
        total += n
    bump[5]()
    print(total)
