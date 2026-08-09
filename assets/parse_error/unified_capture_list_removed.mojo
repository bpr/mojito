# Current Mojo removed the `unified` keyword; a capture list is a bare `{...}`
# after the effects clause. The legacy spelling is rejected, not normalized.
# expect: 'unified {...}' capture spelling is not accepted
def counter() -> Int:
    var total: Int = 0
    def add(x: Int) unified {mut total}:
        total = total + x
    add(5)
    return total

def main():
    print(counter())
