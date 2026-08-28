# expect: access to 'source' conflicts with live reference 'v'
# An owned place auto-borrowed into a ref ctor parameter installs the same
# source loan an explicit `ref` binding would: mutating the source while the
# view lives is rejected.
@fieldwise_init
struct View[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]
    var index: Int

    def first(self) -> Int:
        return self.src[self.index]

def main():
    var source = List[Int]()
    source.append(7)
    var v = View(source, 0)
    source.append(9)
    print(v.first())
