# Mojito accepts binding tracked local places to ref[ImmUntrackedOrigin] /
# ref[MutUnsafeAnyOrigin] parameters; the audited head rejects the conversion
# ('Int' cannot convert to an untracked ref) — a recorded acceptance
# divergence.
def observe_static(ref[ImmStaticOrigin] value: Int):
    print(value)

def observe_untracked(ref[ImmUntrackedOrigin] value: Int):
    print(value)

def mutate_unsafe(ref[MutUnsafeAnyOrigin] value: Int):
    value += 1

def main():
    var value = 41
    observe_untracked(value)
    mutate_unsafe(value)
    print(value)
