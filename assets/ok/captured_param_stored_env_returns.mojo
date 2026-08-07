# A capture rooted at a caller-owned parameter place may leave the frame
# inside stored callable storage: parameter origins do not escape.
@fieldwise_init
struct Env:
    var callback: def() capturing[_] -> Int

def wrap(mut n: Int) -> Env:
    def peek() unified {imm n} -> Int:
        return n
    return Env(peek)

def main():
    var value = 4
    var env = wrap(value)
    print(1)
