struct Part(Movable):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def __deinit__(deinit self):
        print("drop part", self.id)


struct Proof[o: Origin[mut=False]](Movable):
    var source: ref[o] Int
    var exact: Int
    var part: Part

    def __init__(out self, ref [Self.o] source: Int, id: Int):
        self.source = source
        self.exact = 9007199254740993
        self.part = Part(id)

    def read[k: Int](self) -> Int:
        return self.exact + self.source + k

    def __deinit__(deinit self):
        print("drop proof")


def risky(fail: Bool) raises:
    print("call")
    if fail:
        raise Error("a1")
    print("ok")


def exercise(fail: Bool, id: Int):
    var source: Int = 10
    try:
        var original = Proof(source, id)
        var moved = original^
        print(moved.read[1](), moved.read[2]())
        try:
            risky(fail)
            print("after", moved.read[1]())
        finally:
            print("finally")
    except:
        print("caught")
    print("end", id)


def main():
    exercise(False, 1)
    exercise(True, 2)
