# A `mut self`/`ref self` method may read a `Pointer[<aggregate>, origin]`
# field's referent — subscript it, take its length — exactly as `def self` does.
# A borrowed receiver is a runtime alias, so loading the field yielded its stored
# handle instead of the referent, and the nominal subscript/`len` then saw a
# reference ("checked nominal subscript receiver is ref"). The field dereference
# now reaches the referent under every receiver convention.
@fieldwise_init
struct Cursor[o: Origin[mut=False]]:
    var src: Pointer[List[Int], Self.o]
    var index: Int

    def next_value(mut self) -> Int:
        var value = self.src[][self.index]
        self.index += 1
        return value

    def remaining(ref self) -> Int:
        return len(self.src[]) - self.index


@fieldwise_init
struct Peek[o: Origin[mut=False]]:
    var src: Pointer[List[Int], Self.o]

    def get(ref self, i: Int) -> Int:
        return self.src[][i]


def main():
    var xs = List[Int]()
    xs.append(10)
    xs.append(20)
    xs.append(30)

    var cursor = Cursor(Pointer(to=xs), 0)
    print(cursor.remaining())   # 3   (ref self: len)
    print(cursor.next_value())  # 10  (mut self: subscript)
    print(cursor.next_value())  # 20
    print(cursor.remaining())   # 1

    var peek = Peek(Pointer(to=xs))
    print(peek.get(2))          # 30  (ref self: subscript)
