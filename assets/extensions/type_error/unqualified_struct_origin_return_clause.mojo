# expect: unqualified access to struct parameter 'o'; use 'Self.o' instead
# The return-clause twin of the parameter-clause rejection: a bare struct
# origin binder in a ref-return clause requires the qualified spelling.
struct Box[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] Int

    def __init__(out self, ref [Self.o] value: Int):
        self.src = value

    def get(self) -> ref[o] Int:
        return self.src

def main():
    pass
