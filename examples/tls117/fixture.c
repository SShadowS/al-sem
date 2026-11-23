/* TLS-117: the PE fixture for the probe's canonicaliser tests (`examples/tls117_probe.rs`,
   module `pe`). It has relocated pointers in read-only data (`table`, `names` in .rdata: DIR64
   base relocations), an import (an IAT in the read-only .idata), and initialised mutable data
   (`counter` in .data), and a TLS-117 data-digest slot (`tls117_slot` in .rdata: a 16-byte
   magic, then 32 zero bytes until stamped). The magic is a test-only spelling
   (`TLS117-FIXT-SLOT`), never the probe's, so embedding this file in the probe's test binary
   does not give the probe's magic a second copy. Built with mingw-w64 GCC 13
   (x86_64-w64-mingw32-gcc):

   x86_64-w64-mingw32-gcc -O1 -nostdlib -ffreestanding -Wl,--dynamicbase -Wl,--high-entropy-va \
       -Wl,-e,entry -s -o fixture.exe fixture.c -lkernel32
*/
__declspec(dllimport) unsigned long __stdcall GetTickCount(void);
static int one(void) { return 1; }
static int two(void) { return 2; }
int (*const table[])(void) = { one, two, one };
const char *const names[] = { "one", "two" };
__attribute__((used)) const unsigned char tls117_slot[48] = "TLS117-FIXT-SLOT";
int counter = 5;
int entry(void) { counter += table[counter & 1]() + (int)names[1][0] + tls117_slot[counter & 15]; return counter + (int)GetTickCount(); }
