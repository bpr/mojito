# Reborrowing a ref FIELD into a local `ref` binding reads through the stored
# handle; a parametric-origin receiver classifies as an immutable access, so
# the reborrow does not conflict with its own loan. Both receiver spellings.
@fieldwise_init
struct View[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]

    def first(self) -> Int:
        ref s = self.src
        return s[0]

    def last(ref self) -> Int:
        ref s = self.src
        return s[len(s) - 1]

def main():
    var data = List[Int]()
    data.append(9)
    data.append(5)
    ref r = data
    var v = View(r)
    print(v.first())
    print(v.last())
    print(v.src[0] + v.first())
