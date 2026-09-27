# A struct built over arithmetic on the enclosing struct's value parameter
# (`Counter[1 + Self.length](self.i)`) fails MIR verification on both
# backends: "place rooted at slot 0 lacks complete checked type metadata".
# Filed from the forwarded value argument work (roadmap section 3); the
# pinned Mojo runs it.
struct Counter[length: Int](Copyable, Movable):
    var i: Int

    def __init__(out self, i: Int):
        self.i = i

    def wider(self) -> Int:
        return Counter[1 + Self.length](self.i).i


def main():
    print(Counter[4](1).wider())
