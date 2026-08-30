# Both compilers reject a bare struct origin binder in a member origin
# clause: the a79fbdf59f2 pin reports "unqualified access to struct
# parameter 'o'; use 'Self.o' instead", and Mojito reports the same
# message (clause tightening, 2026-08-30).
struct View[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]

    def __init__(out self, ref [o] src: List[Int]):
        self.src = Pointer(to=src)

def main():
    pass
