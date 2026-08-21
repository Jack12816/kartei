# frozen_string_literal: true

RSpec.describe User do
  it 'builds the display name' do
    expect(described_class.new.display_name).to be_a(String)
  end
end
