# Origin arguments through comptime alias channels: an alias body binding its
# own origin parameter, a parameterized alias member applied in field
# position, and upstream dict.mojo's monomorphic-alias-in-field shape.
@fieldwise_init
struct EntryIter[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]
    var index: Int

    def next_val(mut self) -> Int:
        var r = self.index
        self.index += 1
        return self.src[r]

@fieldwise_init
struct KeyIter[m: Bool, //, o: Origin[mut=m]]:
    comptime dict_entry_iter = EntryIter[Self.o]

    var iter: Self.dict_entry_iter

    def next_val(mut self) -> Int:
        return self.iter.next_val()

@fieldwise_init
struct ValueIter[m: Bool, //, o: Origin[mut=m]]:
    comptime InnerType[vm: Bool, //, vo: Origin[mut=vm]] = EntryIter[vo]

    var iter: Self.InnerType[o]

    def next_val(mut self) -> Int:
        return self.iter.next_val()

@fieldwise_init
struct Pane[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]
    var index: Int

    def first(self) -> Int:
        return self.src[0]

struct Box:
    comptime PaneType[vm: Bool, //, vo: Origin[mut=vm]] = Pane[vo]

    var items: List[Int]

    def __init__(out self):
        self.items = List[Int]()
        self.items.append(21)

    def pane(ref self) -> Self.PaneType[origin_of(self)]:
        ref source = self.items
        return Pane(source, 0)

def main():
    var data = List[Int]()
    data.append(1)
    data.append(2)
    ref r = data
    var k = KeyIter(EntryIter(r, 0))
    print(k.next_val())
    var v = ValueIter(EntryIter(r, 1))
    print(v.next_val())
    var b = Box()
    var p = b.pane()
    print(p.first())
