# Temporaries passed straight to `Writer.write` at their source's last use:
# a subscript view keeps its source alive through the call, and a `Named`
# temporary holding a pointer to a caller local reads it (upstream's
# temporary lifetime). Native `Writer.write` takes neither a `StringSpan` nor
# a struct argument yet, so these shapes are pinned against the pinned Mojo
# on the VM only.
from std.format._utils import Named

def main():
    var s = String("hello")
    var out = String()
    out.write(s[byte=1:3])
    print(out)
    var w = 7
    var text = String()
    text.write(Named("n", w))
    print(text)
