# expect: unqualified access to struct parameter 'o'; use 'Self.o' instead
# A struct origin parameter referenced from a member origin clause requires
# the qualified spelling (pin-attested); the bare binder rejects in both
# parameter and return clause positions.
struct Holder[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]

    def __init__(out self, ref [o] src: List[Int]):
        self.src = src

def main():
    pass
