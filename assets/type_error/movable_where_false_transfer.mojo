# A declared conditional `Movable` opt-out makes `^` transfer reject: the
# value can be neither rebound nor moved into storage.
# expect: does not conform to trait 'Movable'
struct Pinned(Movable where False):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

def main():
    var p = Pinned(1)
    var q = p^
    print(q.id)
