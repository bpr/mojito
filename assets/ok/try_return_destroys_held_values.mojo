# A `return` that leaves through a `try` destroys what it still holds: a
# value the loop after the `try` needs dies before the return leaves, and a
# value the `finally` reads dies after the `finally` runs.


struct Noisy(Movable):
    var tag: Int

    def __init__(out self, tag: Int):
        self.tag = tag

    def __deinit__(deinit self):
        print("drop", self.tag)


def may(i: Int) raises:
    if i == 7:
        raise Error("seven")


def no_finally(n: Int) -> Int:
    var s = Noisy(1)
    var i = 0
    while i < 3:
        try:
            may(i)
            if i == n:
                print("returning")
                return i
        except:
            print("caught")
        i += 1
    print("after", s.tag)
    return -1


def through_finally(n: Int) -> Int:
    var s = Noisy(2)
    var i = 0
    while i < 3:
        try:
            if i == n:
                print("returning")
                return i
        finally:
            print("finally", s.tag)
        i += 1
    print("after", s.tag)
    return -1


def main():
    print(no_finally(1))
    print(no_finally(9))
    print(through_finally(1))
    print(through_finally(9))
