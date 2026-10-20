d="${HOME:-/var/tmp}/.cache/fastctx-install/v@VERSION@/$a-@TARGET@";
mkdir -p "$d"; cd "$d";
check(){ echo "$h  a"|@CHECKSUM@ -c - >/dev/null; };
command -v curl >/dev/null 2>&1 || { echo 'fastctx: curl is missing; install curl and run this installation line again.' >&2; exit 1; };
started=$(date +%s); attempts=0; http_errors=0; http_started=0;
download(){
  w=1;
  while :; do
    now=$(date +%s); left=$((1200-now+started));
    if [ "$left" -le 0 ] || [ "$attempts" -ge 24 ]; then
      echo 'fastctx: download did not finish within the retry limit; check the network and run this line again to resume.' >&2; exit 1;
    fi;
    if [ "$http_errors" -gt 0 ]; then
      http_left=$((180-now+http_started));
      if [ "$http_left" -le 0 ]; then echo 'fastctx: the server keeps returning an HTTP error; check the release address or ask its administrator, then run the line again.' >&2; exit 1; fi;
      [ "$left" -le "$http_left" ] || left=$http_left;
    fi;
    maximum=120; [ "$left" -ge "$maximum" ] || maximum=$left;
    attempts=$((attempts+1));
    if status=$(curl --proto '=https' -fL --connect-timeout 15 --max-time "$maximum" -C - -o a --write-out '%{http_code}' @OPTIONS@ @URL@/fastctx-$a-@TARGET@.tar.gz); then return 0; else r=$?; fi;
    case $r in
      5|6|7|16|18|28|35|52|55|56|92) http_errors=0;;
      33) http_errors=0; rm -f a;;
      22) case $status in 400|401|403|404|410) echo "fastctx: the release address returned HTTP $status; obtain a fresh installation line or ask its administrator." >&2; exit 1;; esac; [ "$http_errors" -gt 0 ] || http_started=$(date +%s); http_errors=$((http_errors+1)); if [ "$http_errors" -ge 8 ]; then echo 'fastctx: the server keeps returning an HTTP error; check the release address or ask its administrator, then run the line again.' >&2; exit 1; fi;;
      1|2|3|4) echo "fastctx: curl cannot use this download command (curl $r); check the address and curl installation, then obtain a fresh installation line." >&2; exit 1;;
      126|127) echo 'fastctx: curl could not be executed; install a working curl and run this line again.' >&2; exit 1;;
      *) echo "fastctx: download failed (curl $r); correct the error printed above and run this line again." >&2; exit 1;;
    esac;
    now=$(date +%s); left=$((1200-now+started)); [ "$left" -gt 0 ] || continue;
    pause=$w; [ "$left" -ge "$pause" ] || pause=$left;
    echo "download stopped (curl $r); again in $pause s" >&2;
    sleep "$pause"; w=$((w*2)); [ "$w" -le 60 ] || w=60;
  done;
};
if ! check 2>/dev/null; then
  download;
  if ! check; then
    rm -f a; download;
    check || { echo 'fastctx: the downloaded bytes do not match the release; obtain a fresh installation line or ask its administrator.' >&2; exit 1; };
  fi;
fi
