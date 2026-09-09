# Run the template's actual integration test without Homebrew installation or host edits.
# This small DSL shim is not a substitute for `brew audit`/`brew test` release acceptance.
require "digest"
require "etc"
require "fileutils"
require "pathname"
require "open3"
require "tmpdir"

abort "Usage: ruby scripts/test-homebrew-formula.rb NATIVE_BINARY" unless ARGV.length == 1
source = Pathname.new(ARGV.fetch(0)).realpath
abort "Expected an executable native binary" unless source.file? && source.executable?

module Utils
  def self.safe_popen_read(*args)
    output, status = Open3.capture2(*args.map(&:to_s))
    raise "Command failed: #{args.inspect}" unless status.success?

    output
  end
end

class Formula
  def self.desc(_value); end
  def self.homepage(_value); end
  def self.version(_value); end
  def self.license(_value); end
  def self.depends_on(_value); end
  def self.on_macos; end
  def self.on_linux; end

  def self.test(&block)
    define_method(:run_test, &block)
  end

  attr_reader :testpath, :opt_bin

  def initialize(root)
    @testpath = root/"test"
    @testpath.mkpath
    @opt_bin = root/"brew/opt/agent-float-term/bin"
  end

  def system(*args)
    raise "Command failed: #{args.inspect}" unless Kernel.system(*args.map(&:to_s))
  end

  def assert_equal(expected, actual)
    raise "Expected #{expected.inspect}, got #{actual.inspect}" unless expected == actual
  end

  def assert_match(expected, actual)
    raise "Missing #{expected.inspect} in #{actual.inspect}" unless actual.include?(expected)
  end

  def assert_path_exists(path)
    raise "Expected path: #{path}" unless path.exist?
  end

  def refute_path_exists(path)
    raise "Unexpected path: #{path}" if path.exist? || path.symlink?
  end
end

load File.join(__dir__, "agent-float-term.rb.in")
raise "Formula must not write user state during Homebrew installation" if AgentFloatTerm.instance_methods(false).include?(:post_install)
# Short private socket paths also work within macOS's sockaddr_un limit.
Dir.mktmpdir("aft-formula-", "/tmp") do |directory|
  root = Pathname.new(directory).realpath
  prefix = root/"brew"
  keg = prefix/"Cellar/agent-float-term/0.1.0"
  (keg/"bin").mkpath
  FileUtils.copy_file(source, keg/"bin/agent-float-term")
  File.chmod(0o755, keg/"bin/agent-float-term")
  (prefix/"opt").mkpath
  File.symlink(keg, prefix/"opt/agent-float-term")
  if RUBY_PLATFORM.include?("darwin")
    admin = Etc.getgrnam("admin").gid
    if Process.groups.include?(admin)
      # Homebrew's package ancestry is commonly current-user:admin and 0775.
      [prefix/"opt", prefix/"Cellar", keg.parent, keg, keg/"bin"].each do |path|
        File.chown(nil, admin, path)
        File.chmod(0o775, path)
      end
      puts "Testing group-writable admin-owned Homebrew package directories."
    end
  else
    [prefix, prefix/"opt", prefix/"Cellar", keg.parent, keg, keg/"bin"].each do |path|
      File.chown(nil, Process.egid, path)
      File.chmod(0o775, path)
    end
    puts "Testing group-writable primary-group Homebrew package directories."
  end
  AgentFloatTerm.new(root).run_test
end
puts "Formula integration test passed (isolated DSL shim; no Homebrew invoked)."
