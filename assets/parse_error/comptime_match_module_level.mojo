# expect: 'comptime match' must be contained in a function
# Upstream requires a `comptime match` inside a function; at module level it
# rejects with this sentence.
comptime N = 4

comptime match N:
    case 4:
        comptime M = 1

def main():
    print(N)
