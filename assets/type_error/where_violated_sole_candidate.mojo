# expect: violated constraint; constraint declared here evaluated to False, expected 'conforms_to(T, Copyable)'
# A sole-candidate call whose message-less `where` clause is false reports
# upstream's violated-constraint note, naming the clause as declared.
struct Plain:
    var v: Int

    def __init__(out self):
        self.v = 0

struct Box[T: AnyType]:
    var n: Int

    def __init__(out self):
        self.n = 0

    def show(self) -> Int where conforms_to(Self.T, Copyable):
        return self.n

def main():
    var b = Box[Plain]()
    print(b.show())
