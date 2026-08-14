# expect: no overload matches
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
    var displaced = d.insert(1, Conn(1))
