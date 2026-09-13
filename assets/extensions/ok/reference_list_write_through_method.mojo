# An augmented `List[ref T]` element write inside a `mut self` method reaches
# the stored referent in caller-owned storage through the properly bound
# origin parameter — sound aliasing, not an escape.
@fieldwise_init
struct RefList[origin: Origin[mut=True]]:
    var values: List[ref[origin] Int]
    def bump_first(mut self):
        self.values[0] += 2

def main():
    var keep = 4
    ref a = keep
    var refs = RefList([a])
    refs.bump_first()
    print(keep)
