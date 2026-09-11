package protocol

import (
	"bufio"
	"errors"
	"fmt"
	"io"
	"strconv"
	"strings"
	"unicode/utf8"
)

const Magic = "SLURP\x00\x01\x00"

type Limits struct{ MaxFileBytes, MaxFiles, MaxTotalBytes, MaxMetadataBytes uint64 }
type Artifact struct {
	Path, SourceMutationToken string
	Size                      uint64
	Body                      io.Reader
}
type Result struct {
	Status, Detail, Version string
	Files, Bytes, Unchanged uint64
}

func field(r *bufio.Reader, max int) (string, error) {
	b, err := r.ReadSlice(0)
	if errors.Is(err, bufio.ErrBufferFull) {
		return "", fmt.Errorf("protocol metadata exceeds %d bytes", max)
	}
	if err != nil {
		return "", fmt.Errorf("read protocol field: %w", err)
	}
	if len(b)-1 > max || !utf8.Valid(b[:len(b)-1]) {
		return "", fmt.Errorf("invalid protocol field")
	}
	return string(b[:len(b)-1]), nil
}

func Decode(src io.Reader, limits Limits, put func(Artifact) error) (Result, error) {
	r := bufio.NewReaderSize(src, 64<<10)
	magic := make([]byte, len(Magic))
	if _, err := io.ReadFull(r, magic); err != nil {
		return Result{}, fmt.Errorf("read protocol magic: %w", err)
	}
	if string(magic) != Magic {
		return Result{}, fmt.Errorf("unsupported remote protocol")
	}
	seen := map[string]bool{}
	var out Result
	metadataLimit := limits.MaxMetadataBytes
	if metadataLimit == 0 {
		metadataLimit = 64 << 20
	}
	var metadataBytes uint64
	for {
		tag, err := r.ReadByte()
		if err != nil {
			return Result{}, fmt.Errorf("protocol ended before end record: %w", err)
		}
		nul, err := r.ReadByte()
		if err != nil || nul != 0 {
			return Result{}, fmt.Errorf("malformed protocol record")
		}
		switch tag {
		case 'F':
			if out.Status != "" {
				return Result{}, fmt.Errorf("artifact after terminal status")
			}
			p, err := field(r, 16<<10)
			if err != nil {
				return Result{}, err
			}
			ss, err := field(r, 64)
			if err != nil {
				return Result{}, err
			}
			ck, err := field(r, 128)
			if err != nil {
				return Result{}, err
			}
			sz, err := strconv.ParseUint(ss, 10, 64)
			if err != nil {
				return Result{}, fmt.Errorf("invalid artifact length")
			}
			if _, err := strconv.ParseUint(ck, 10, 64); err != nil {
				return Result{}, fmt.Errorf("invalid source checksum")
			}
			if seen[p] {
				return Result{}, fmt.Errorf("duplicate artifact path %q", p)
			}
			retained := uint64(len(p))*2 + uint64(len(ck)) + 512
			if ^uint64(0)-metadataBytes < retained || metadataBytes+retained > metadataLimit {
				return Result{}, fmt.Errorf("protocol metadata limit exceeded")
			}
			metadataBytes += retained
			seen[p] = true
			if limits.MaxFiles > 0 && out.Files >= limits.MaxFiles {
				return Result{}, fmt.Errorf("artifact count limit exceeded")
			}
			if limits.MaxFileBytes > 0 && sz > limits.MaxFileBytes {
				return Result{}, fmt.Errorf("artifact %q exceeds file size limit", p)
			}
			if ^uint64(0)-out.Bytes < sz || limits.MaxTotalBytes > 0 && out.Bytes+sz > limits.MaxTotalBytes {
				return Result{}, fmt.Errorf("total byte limit exceeded")
			}
			lr := &io.LimitedReader{R: r, N: int64(sz)}
			if sz > uint64(^uint64(0)>>1) {
				return Result{}, fmt.Errorf("artifact too large")
			}
			if err := put(Artifact{Path: p, SourceMutationToken: ck, Size: sz, Body: lr}); err != nil {
				return Result{}, err
			}
			if lr.N != 0 {
				return Result{}, fmt.Errorf("artifact %q body not fully consumed", p)
			}
			out.Files++
			out.Bytes += sz
		case 'T':
			if out.Status != "" {
				return Result{}, fmt.Errorf("duplicate terminal status")
			}
			out.Status, err = field(r, 64)
			if err != nil {
				return Result{}, err
			}
			out.Detail, err = field(r, 64<<10)
			if err != nil {
				return Result{}, err
			}
			out.Version, err = field(r, 4<<10)
			if err != nil {
				return Result{}, err
			}
			if out.Status != "ok" && out.Status != "not-found" && out.Status != "failed" {
				return Result{}, fmt.Errorf("invalid terminal status %q", out.Status)
			}
		case 'E':
			if out.Status == "" {
				return Result{}, fmt.Errorf("end before terminal status")
			}
			if b, err := r.ReadByte(); err != io.EOF {
				if err == nil {
					_ = b
				}
				return Result{}, fmt.Errorf("trailing protocol data")
			}
			return out, nil
		default:
			return Result{}, fmt.Errorf("unknown protocol record %q", tag)
		}
	}
}

func ValidateRelativePath(p string) error {
	if p == "" || strings.HasPrefix(p, "/") || strings.ContainsRune(p, 0) || !utf8.ValidString(p) {
		return fmt.Errorf("invalid relative path")
	}
	for _, part := range strings.Split(p, "/") {
		if part == "" || part == "." || part == ".." {
			return fmt.Errorf("unsafe relative path %q", p)
		}
	}
	return nil
}
