# expect: doesn't match expected origin 'ImmUntrackedOrigin'
# A `ref[ImmUntrackedOrigin]` (or `ImmStaticOrigin` / `MutUntrackedOrigin`)
# parameter binds only an actual carrying that exact origin; a tracked local
# place does not convert. Both compilers reject (pinned Mojo a79fbdf59f2:
# "value passed to 'value' cannot be converted from 'Int' to ref 'Int'").
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
