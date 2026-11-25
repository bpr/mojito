# expect: invalidated interior reference
# Appending through a `ref` field while a loop iterates the same field in a
# `mut self` method is rejected: the field's origin is the struct's origin
# parameter, so the loop's interior generation and the write both name the
# field's own place, as a write through the field's owner does.
@fieldwise_init
struct Walk[o: MutOrigin]:
    var src: ref[Self.o] List[Int]

    def dup(mut self):
        for x in self.src:
            self.src.append(x)

def main():
    var l: List[Int] = [1]
    var w = Walk(l)
    w.dup()
    print(len(l))
