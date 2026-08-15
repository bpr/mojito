# Displacement-returning `insert` no longer requires `Deinitable` (the
# audited head's contract: nothing is destroyed in place, the displaced
# entry moves out). The fresh-key path runs for a linear value type; the
# container tears down through `deinit_with`. Consuming a displaced
# DictEntry's linear `value` field is a recorded VM gap (interior field
# consumption of a generic entry value).
@explicit_destroy("close Conn")
struct Conn(Equatable, Movable, Copyable, Deinitable where False):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def __eq__(self, other: Self) -> Bool:
        return self.id == other.id

    def __ne__(self, other: Self) -> Bool:
        return self.id != other.id

    def close(deinit self):
        print("close", self.id)

def main():
    var d: Dict[Int, Conn] = Dict[Int, Conn]()
    var fresh = d.insert(1, Conn(1))
    fresh^.deinit_assert_empty()
    d^.deinit_with(lambda (var key: Int, var value: Conn): value^.close())
    print("done")
