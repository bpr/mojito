# Both implementations reject returning a pointer whose inferred origin is a
# local as the untracked heap origin: the pinned nightly reports that the
# local's origin does not convert to `MutUntrackedOrigin`, and Mojito reports
# the escape directly.
def escape() -> Pointer[Int, MutUntrackedOrigin]:
    var local = 7
    return Pointer(to=local)

def main():
    print(1)
