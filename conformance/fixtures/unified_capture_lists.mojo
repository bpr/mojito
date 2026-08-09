# The removed `unified` keyword is rejected by both compilers; the current
# capture list is a bare `{...}` after the effects clause.
def captured_total() -> Int:
    var total = 0
    def add() unified {mut total}:
        total = total + 1
    add()
    return total

def main():
    print(captured_total())
